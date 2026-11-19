#[cfg(not(feature = "sim"))]
#[test]
fn std_roundtrip_and_lock() {
    let path = std::env::temp_dir().join(format!("sled-fs-proof-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    let db = sled::Config::new().path(&path).open().unwrap();
    assert!(sled::Config::new().path(&path).open().is_err());
    db.insert(b"key", b"value").unwrap();
    db.insert(b"blob", vec![42; 100_000]).unwrap();
    db.flush().unwrap();
    drop(db);
    let db = sled::open(&path).unwrap();
    assert_eq!(db.get(b"key").unwrap().unwrap().as_ref(), b"value");
    assert_eq!(db.get(b"blob").unwrap().unwrap().len(), 100_000);
    drop(db);
    std::fs::remove_dir_all(path).unwrap();
}

#[cfg(feature = "sim")]
mod simulated {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use turmoil::fs::shim::std::fs;

    fn config(path: &str) -> sled::Config {
        sled::Config::new()
            .path(path)
            .flush_every_ms(None)
            .cache_capacity(1024 * 1024)
            .segment_size(4096)
    }

    #[test]
    fn roundtrip_blobs_and_host_isolation() -> turmoil::Result {
        for seed in [7, 42, 314] {
            let mut sim = turmoil::Builder::new().rng_seed(seed).build();
            for (host, value) in [("alpha", b"alpha"), ("bravo", b"bravo")] {
                sim.client(host, async move {
                    let db = config("/database").open()?;
                    assert!(db.get("key")?.is_none());
                    db.insert("key", value)?;
                    db.insert("blob", vec![19; 20_000])?;
                    db.flush()?;
                    assert_eq!(db.get("key")?.unwrap().as_ref(), value);
                    assert!(db.size_on_disk()? > 20_000);
                    assert!(fs::metadata("/database/db")?.len() > 0);
                    assert!(fs::read_dir("/database/blobs")?.next().is_some());
                    drop(db);
                    let db = config("/database").open()?;
                    assert_eq!(db.get("key")?.unwrap().as_ref(), value);
                    assert_eq!(db.get("blob")?.unwrap().as_ref(), vec![19; 20_000]);
                    drop(db);
                    Ok(())
                });
            }
            sim.run()?;
        }
        Ok(())
    }

    #[test]
    fn eio_reaches_real_sled_config_write() -> turmoil::Result {
        for probability in [0.0, 1.0] {
            let mut builder = turmoil::Builder::new();
            builder.rng_seed(314).fs().io_error_probability(probability);
            let mut sim = builder.build();
            sim.client("host", async move {
                let result = config("/eio").open();
                if probability == 0.0 {
                    drop(result?);
                } else {
                    match result {
                        Err(sled::Error::Io(e)) => {
                            assert!(e.to_string().contains("I/O error"), "{e:?}")
                        }
                        other => panic!("expected EIO from sled config write: {other:?}"),
                    }
                }
                Ok(())
            });
            sim.run()?;
        }
        Ok(())
    }

    #[test]
    fn enospc_reaches_real_sled_log_flush() -> turmoil::Result {
        for capacity in [16_384, 1_000_000] {
            let mut builder = turmoil::Builder::new();
            builder.rng_seed(314).fs().capacity(capacity);
            let mut sim = builder.build();
            sim.client("host", async move {
                let db = config("/full").open()?;
                db.flush()?;
                // Consume free space through the simulated disk, not a sled failpoint.
                let filler = fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .open("/filler")?;
                filler.set_len(12_000)?;
                let mut result = Ok(0);
                for i in 0u32..300 {
                    db.insert(i.to_be_bytes(), vec![17; 100])?;
                    result = db.flush();
                    if result.is_err() {
                        break;
                    }
                }
                if capacity == 16_384 {
                    match result {
                        Err(sled::Error::Io(e)) => {
                            assert!(e.to_string().contains("No space left"), "{e:?}")
                        }
                        other => panic!("expected ENOSPC in sled log I/O: {other:?}"),
                    }
                    // Failed Db drop can itself retry I/O; this test models abrupt disposal.
                    std::mem::forget(db);
                } else {
                    result?;
                    drop(db);
                }
                Ok(())
            });
            sim.run()?;
        }
        Ok(())
    }

    #[test]
    fn crash_recovery_without_destructor_flush() -> turmoil::Result {
        for seed in [7, 42, 314] {
            for (flush_last, explicit_sync) in [(false, true), (true, true), (true, false)] {
                let mut builder = turmoil::Builder::new();
                builder.rng_seed(seed).fs().sync_probability(0.0);
                let mut sim = builder.build();
                let phase = Arc::new(AtomicUsize::new(0));
                let completed = Arc::new(AtomicUsize::new(0));
                let p = phase.clone();
                let c = completed.clone();
                sim.host("host", move || {
                    let p = p.clone();
                    let c = c.clone();
                    async move {
                        let db = config("/crash").open()?;
                        if p.load(Ordering::SeqCst) == 0 {
                            db.insert("committed", b"yes")?;
                            db.flush()?;
                            // Both the adapter-only and explicitly synced cases must recover.
                            if explicit_sync {
                                fs::OpenOptions::new()
                                    .write(true)
                                    .open("/crash/conf")?
                                    .sync_all()?;
                                fs::sync_dir("/crash/blobs")?;
                                fs::sync_dir("/crash")?;
                                fs::sync_dir("/")?;
                            }
                            db.insert("last", b"value")?;
                            assert_eq!(db.get("last")?.unwrap().as_ref(), b"value");
                            if flush_last {
                                db.flush()?;
                            }
                            // A plain Sim::crash drops futures, which drops Db and flushes.
                            // Leak this bounded, worker-free instance BEFORE yielding so no
                            // destructor can persist it during crash or later teardown.
                            std::mem::forget(db);
                        } else {
                            assert_eq!(db.get("committed")?.unwrap().as_ref(), b"yes");
                            assert_eq!(db.get("last")?.is_some(), flush_last);
                            drop(db);
                        }
                        c.fetch_add(1, Ordering::SeqCst);
                        std::future::pending::<()>().await;
                        Ok(())
                    }
                });
                for _ in 0..10 {
                    sim.step()?;
                    if completed.load(Ordering::SeqCst) == 1 {
                        break;
                    }
                }
                assert_eq!(completed.load(Ordering::SeqCst), 1);
                sim.crash("host");
                phase.store(1, Ordering::SeqCst);
                sim.bounce("host");
                for _ in 0..10 {
                    sim.step()?;
                    if completed.load(Ordering::SeqCst) == 2 {
                        break;
                    }
                }
                assert_eq!(completed.load(Ordering::SeqCst), 2);
            }
        }
        Ok(())
    }

    #[test]
    fn crash_discards_pending_blob_bytes_written_by_sled() -> turmoil::Result {
        let mut builder = turmoil::Builder::new();
        builder.rng_seed(314).fs().sync_probability(0.0);
        let mut sim = builder.build();
        let completed = Arc::new(AtomicUsize::new(0));
        let c = completed.clone();
        sim.host("host", move || {
            let c = c.clone();
            async move {
                if c.load(Ordering::SeqCst) == 0 {
                    let db = config("/blob-crash").open()?;
                    db.insert("committed", b"yes")?;
                    db.flush()?;
                    fs::OpenOptions::new()
                        .write(true)
                        .open("/blob-crash/conf")?
                        .sync_all()?;
                    fs::sync_dir("/blob-crash")?;
                    fs::sync_dir("/")?;
                    db.insert("pending-blob", vec![29; 20_000])?;
                    let blobs: Vec<_> =
                        fs::read_dir("/blob-crash/blobs")?.collect::<Result<_, _>>()?;
                    assert_eq!(blobs.len(), 1);
                    assert!(fs::read(blobs[0].path())?.len() > 20_000);
                    // Persist its name, deliberately leave the actual sled-written bytes pending.
                    fs::sync_dir("/blob-crash/blobs")?;
                    std::mem::forget(db);
                } else {
                    let blobs: Vec<_> =
                        fs::read_dir("/blob-crash/blobs")?.collect::<Result<_, _>>()?;
                    assert_eq!(blobs.len(), 1);
                    assert!(fs::read(blobs[0].path())?.is_empty());
                    let db = config("/blob-crash").open()?;
                    assert_eq!(db.get("committed")?.unwrap().as_ref(), b"yes");
                    assert!(db.get("pending-blob")?.is_none());
                    assert!(fs::read_dir("/blob-crash/blobs")?.next().is_none());
                    drop(db);
                }
                c.fetch_add(1, Ordering::SeqCst);
                std::future::pending::<()>().await;
                Ok(())
            }
        });
        for _ in 0..10 {
            sim.step()?;
            if completed.load(Ordering::SeqCst) == 1 {
                break;
            }
        }
        assert_eq!(completed.load(Ordering::SeqCst), 1);
        sim.crash("host");
        sim.bounce("host");
        for _ in 0..10 {
            sim.step()?;
            if completed.load(Ordering::SeqCst) == 2 {
                break;
            }
        }
        assert_eq!(completed.load(Ordering::SeqCst), 2);
        Ok(())
    }

    #[test]
    fn rejects_background_flusher_and_relative_paths() -> turmoil::Result {
        let mut sim = turmoil::Builder::new().build();
        sim.client("host", async {
            assert!(sled::Config::new().path("/unsafe").open().is_err());
            assert!(config("relative").open().is_err());
            assert!(config("/temporary").temporary(true).open().is_err());
            Ok(())
        });
        sim.run()
    }
}
