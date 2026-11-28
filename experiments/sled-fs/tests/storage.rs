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
        atomic::{AtomicBool, AtomicUsize, Ordering},
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
            let mut builder = turmoil::Builder::new();
            builder.rng_seed(seed).fs().sync_probability(0.0);
            let mut sim = builder.build();
            let phase = Arc::new(AtomicUsize::new(0));
            let power_lost = Arc::new(AtomicBool::new(false));
            let p = phase.clone();
            let cut = power_lost.clone();
            sim.host("host", move || {
                let p = p.clone();
                let cut = cut.clone();
                async move {
                    if p.load(Ordering::SeqCst) == 0 {
                        let db = config("/crash").simulated_power_loss(cut).open()?;
                        db.insert("committed", b"yes")?;
                        db.insert("durable-blob", vec![19; 20_000])?;
                        db.flush()?;
                        assert!(fs::read_dir("/crash/blobs")?.next().is_some());

                        // The small pending write remains in the log buffer.
                        // No fixture-side sync and no leaked handles.
                        db.insert("unflushed-control", b"must disappear")?;
                        assert!(db.get("unflushed-control")?.is_some());
                        p.store(1, Ordering::SeqCst);
                        std::future::pending::<()>().await;
                        drop(db);
                    } else {
                        let db = config("/crash").open()?;
                        assert_eq!(db.get("committed")?.unwrap().as_ref(), b"yes");
                        assert_eq!(db.get("durable-blob")?.unwrap().as_ref(), vec![19; 20_000]);
                        assert!(db.get("unflushed-control")?.is_none());
                        p.store(2, Ordering::SeqCst);
                    }
                    Ok(())
                }
            });
            for _ in 0..10 {
                sim.step()?;
                if phase.load(Ordering::SeqCst) == 1 { break; }
            }
            assert_eq!(phase.load(Ordering::SeqCst), 1);
            power_lost.store(true, Ordering::Release);
            sim.crash("host");
            sim.bounce("host");
            for _ in 0..10 {
                sim.step()?;
                if phase.load(Ordering::SeqCst) == 2 { break; }
            }
            assert_eq!(phase.load(Ordering::SeqCst), 2);
        }
        Ok(())
    }

    #[test]
    fn crash_recovery_discards_durable_orphan_blob() -> turmoil::Result {
        let mut builder = turmoil::Builder::new();
        builder.rng_seed(314).fs().sync_probability(0.0);
        let mut sim = builder.build();
        let phase = Arc::new(AtomicUsize::new(0));
        let power_lost = Arc::new(AtomicBool::new(false));
        let p = phase.clone();
        let cut = power_lost.clone();
        sim.host("host", move || {
            let p = p.clone();
            let cut = cut.clone();
            async move {
                if p.load(Ordering::SeqCst) == 0 {
                    let db = config("/blob-crash").simulated_power_loss(cut).open()?;
                    db.insert("committed", b"yes")?;
                    db.flush()?;
                    db.insert("pending-blob", vec![29; 20_000])?;
                    p.store(1, Ordering::SeqCst);
                    std::future::pending::<()>().await;
                    drop(db);
                } else {
                    // Blob name and bytes were synced before the reference.
                    let blobs: Vec<_> =
                        fs::read_dir("/blob-crash/blobs")?.collect::<Result<_, _>>()?;
                    assert_eq!(blobs.len(), 1);
                    assert!(fs::read(blobs[0].path())?.len() > 20_000);
                    let db = config("/blob-crash").open()?;
                    assert_eq!(db.get("committed")?.unwrap().as_ref(), b"yes");
                    assert!(db.get("pending-blob")?.is_none());
                    assert!(fs::read_dir("/blob-crash/blobs")?.next().is_none());
                    p.store(2, Ordering::SeqCst);
                }
                Ok(())
            }
        });
        for _ in 0..10 {
            sim.step()?;
            if phase.load(Ordering::SeqCst) == 1 { break; }
        }
        assert_eq!(phase.load(Ordering::SeqCst), 1);
        power_lost.store(true, Ordering::Release);
        sim.crash("host");
        sim.bounce("host");
        for _ in 0..10 {
            sim.step()?;
            if phase.load(Ordering::SeqCst) == 2 { break; }
        }
        assert_eq!(phase.load(Ordering::SeqCst), 2);
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
