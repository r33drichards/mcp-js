//! Bounded ordered compute/copy commands, submitted as one native command buffer.
use super::webgpu::{
    BufferRef, Session, batch_buffers, checked, err, read_buffers, resident_buffers,
    validate_buffer_usages, validate_refs,
};
use deno_error::JsErrorBox;
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Batch {
    #[serde(deserialize_with = "batch_buffers")]
    buffers: Vec<Vec<u8>>,
    refs: Vec<BufferRef>,
    usages: Vec<u32>,
    #[serde(deserialize_with = "bounded_commands")]
    commands: Vec<Command>,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum Command {
    Compute {
        code: String,
        entry: String,
        workgroups: [u32; 3],
        bindings: Vec<Binding>,
    },
    Copy {
        source: usize,
        destination: usize,
        source_offset: u64,
        destination_offset: u64,
        size: u64,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    buffer: usize,
    offset: u64,
    size: u64,
}
fn valid_range(offset: u64, size: u64, length: usize, alignment: u64) -> bool {
    offset % alignment == 0
        && size > 0
        && size % 4 == 0
        && offset
            .checked_add(size)
            .is_some_and(|end| end <= length as u64)
}
fn bounded_commands<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<Command>, D::Error> {
    use serde::de::{Error, SeqAccess, Visitor};
    struct Commands;
    impl<'de> Visitor<'de> for Commands {
        type Value = Vec<Command>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("at most 40 GPU commands")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut commands = Vec::new();
            while let Some(command) = seq.next_element::<Command>()? {
                if commands.len() >= 40 {
                    return Err(A::Error::custom("batch command count"));
                }
                commands.push(command);
            }
            Ok(commands)
        }
    }
    deserializer.deserialize_seq(Commands)
}
impl Batch {
    pub fn validate(&self) -> Result<u32, JsErrorBox> {
        let n = self.buffers.len();
        if n == 0 || n > 16 || self.refs.len() != n || self.usages.len() != n {
            return Err(err("batch resource count"));
        }
        validate_refs(&self.refs, n)?;
        if self
            .buffers
            .iter()
            .any(|b| b.is_empty() || b.len() % 4 != 0)
            || self.buffers.iter().map(Vec::len).sum::<usize>() > 4 * 1024 * 1024
        {
            return Err(err("batch buffer bytes/alignment"));
        }
        validate_buffer_usages(&self.buffers, &self.usages)?;
        if self.commands.is_empty() || self.commands.len() > 40 {
            return Err(err("batch command count"));
        }
        let (mut dispatches, mut copies, mut source_bytes) = (0u32, 0u32, 0usize);
        for c in &self.commands {
            match c {
                Command::Compute {
                    code,
                    entry,
                    workgroups,
                    bindings,
                } => {
                    dispatches += 1;
                    source_bytes = source_bytes
                        .checked_add(code.len())
                        .ok_or_else(|| err("batch shader size"))?;
                    if source_bytes > 65536 || entry.len() > 256 {
                        return Err(err("batch shader limit (64 KiB total)"));
                    }
                    let product = workgroups
                        .iter()
                        .try_fold(1u64, |a, &b| a.checked_mul(b as u64));
                    if workgroups.contains(&0) || product.is_none_or(|p| p > 65536) {
                        return Err(err("workgroup limit"));
                    }
                    let mut unique = std::collections::HashSet::new();
                    if bindings.is_empty()
                        || bindings.len() > 8
                        || bindings.iter().any(|b| {
                            b.buffer >= n
                                || !unique.insert(b.buffer)
                                || self.usages[b.buffer] & (128 | 64) == 0
                                || !valid_range(b.offset, b.size, self.buffers[b.buffer].len(), 256)
                        })
                    {
                        return Err(err("batch binding indices/usage"));
                    }
                }
                Command::Copy {
                    source,
                    destination,
                    source_offset,
                    destination_offset,
                    size,
                } => {
                    copies += 1;
                    if *source >= n
                        || *destination >= n
                        || source == destination
                        || !valid_range(*source_offset, *size, self.buffers[*source].len(), 4)
                        || !valid_range(
                            *destination_offset,
                            *size,
                            self.buffers[*destination].len(),
                            4,
                        )
                        || self.usages[*source] & 4 == 0
                        || self.usages[*destination] & 8 == 0
                    {
                        return Err(err("batch copy bounds/alignment/usage"));
                    }
                }
            }
        }
        if dispatches > 32 || copies > 8 {
            return Err(err("batch work/copy limit"));
        }
        Ok(dispatches.max(1))
    }
    pub fn policy_inputs(&self) -> Vec<serde_json::Value> {
        self.commands.iter().map(|c| match c {
            Command::Compute {workgroups,bindings,..} => serde_json::json!({"operation":"dispatch","bytes":bindings.iter().map(|b| self.buffers[b.buffer].len()).sum::<usize>(),"bindingBytes":bindings.iter().map(|b|b.size).sum::<u64>(),"workgroups":workgroups}),
            Command::Copy {source,destination,source_offset,destination_offset,size} => serde_json::json!({"operation":"copyBuffer","bytes":self.buffers[*source].len()+self.buffers[*destination].len(),"sourceBytes":self.buffers[*source].len(),"destinationBytes":self.buffers[*destination].len(),"copiedBytes":size,"sourceOffset":source_offset,"destinationOffset":destination_offset}),
        }).collect()
    }
}
pub(super) fn run(session: &Session, job: Batch) -> Result<String, JsErrorBox> {
    let _guard = session
        .busy
        .try_lock()
        .map_err(|_| err("device already dispatching"))?;
    let device = &session.device;
    let mut pipelines = Vec::new();
    for command in &job.commands {
        pipelines.push(if let Command::Compute { code, entry, .. } = command {
            Some(checked(device, || {
                let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: None,
                    source: wgpu::ShaderSource::Wgsl(code.clone().into()),
                });
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: None,
                    layout: None,
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                })
            })?)
        } else {
            None
        });
    }
    let buffers = resident_buffers(session, &job.buffers, &job.usages, &job.refs)?;
    let readbacks = checked(device, || {
        let mut encoder = device.create_command_encoder(&Default::default());
        for (command, pipeline) in job.commands.iter().zip(&pipelines) {
            match command {
                Command::Compute {
                    workgroups,
                    bindings,
                    ..
                } => {
                    let pipeline = pipeline.as_ref().unwrap();
                    let entries: Vec<_> = bindings
                        .iter()
                        .enumerate()
                        .map(|(binding, b)| wgpu::BindGroupEntry {
                            binding: binding as u32,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &buffers[b.buffer],
                                offset: b.offset,
                                size: std::num::NonZeroU64::new(b.size),
                            }),
                        })
                        .collect();
                    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: None,
                        layout: &pipeline.get_bind_group_layout(0),
                        entries: &entries,
                    });
                    // Splitting JS dispatches into native passes provides ordered
                    // storage dependencies without any host readback between them.
                    let mut pass = encoder.begin_compute_pass(&Default::default());
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, &group, &[]);
                    pass.dispatch_workgroups(workgroups[0], workgroups[1], workgroups[2]);
                }
                Command::Copy {
                    source,
                    destination,
                    source_offset,
                    destination_offset,
                    size,
                } => encoder.copy_buffer_to_buffer(
                    &buffers[*source],
                    *source_offset,
                    &buffers[*destination],
                    *destination_offset,
                    *size,
                ),
            }
        }
        let readbacks: Vec<_> = buffers
            .iter()
            .map(|b| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: b.size(),
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        for (b, r) in buffers.iter().zip(&readbacks) {
            encoder.copy_buffer_to_buffer(b, 0, r, 0, r.size());
        }
        session.queue.submit([encoder.finish()]);
        readbacks
    })?;
    read_buffers(session, &readbacks)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn payload() -> serde_json::Value {
        serde_json::json!({"buffers":[[0,0,0,0],[0,0,0,0]],"refs":[{"id":1,"revision":0},{"id":2,"revision":0}],"usages":[140,140],"commands":[{"kind":"copy","source":0,"destination":1,"source_offset":0,"destination_offset":0,"size":4}]})
    }
    fn valid(v: &serde_json::Value) -> bool {
        serde_json::from_value::<Batch>(v.clone()).is_ok_and(|b| b.validate().is_ok())
    }
    #[test]
    fn command_bounds() {
        let mut v = payload();
        assert!(valid(&v));
        for dest in [0, 2] {
            v["commands"][0]["destination"] = dest.into();
            assert!(!valid(&v));
        }
        v = payload();
        v["commands"] = vec![v["commands"][0].clone(); 9].into();
        assert!(!valid(&v));
        let compute = serde_json::json!({"kind":"compute","code":"","entry":"main","workgroups":[1,1,1],"bindings":[{"buffer":0,"offset":0,"size":4}]});
        v["commands"] = vec![compute.clone(); 32].into();
        let b: Batch = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(b.validate().unwrap(), 32);
        v["commands"] = vec![compute.clone(); 33].into();
        assert!(!valid(&v));
        v["commands"] = vec![compute; 1].into();
        v["commands"][0]["code"] = "x".repeat(65537).into();
        assert!(!valid(&v));
        v["commands"][0]["code"] = "".into();
        v["usages"][0] = 9.into();
        assert!(!valid(&v));
        v["usages"][0] = 140.into();
        v["commands"][0]["workgroups"] = serde_json::json!([u32::MAX, u32::MAX, u32::MAX]);
        assert!(!valid(&v));
        v["commands"][0]["workgroups"] = serde_json::json!([1, 1, 1]);
        v["commands"][0]["bindings"] = vec![v["commands"][0]["bindings"][0].clone(); 2].into();
        assert!(!valid(&v));
    }
    #[test]
    fn range_overflow_alignment_and_bounds() {
        for (offset, size, length, alignment) in [
            (u64::MAX - 3, 8, 512, 4),
            (0, 0, 512, 4),
            (2, 4, 512, 4),
            (4, 4, 512, 256),
            (0, 3, 512, 4),
            (256, 260, 512, 256),
        ] {
            assert!(!valid_range(offset, size, length, alignment));
        }
        assert!(valid_range(256, 256, 512, 256));
        assert!(valid_range(4, 8, 12, 4));
        for field in ["source_offset", "destination_offset", "size"] {
            let mut v = payload();
            v["commands"][0][field] = u64::MAX.into();
            assert!(!valid(&v));
        }
    }
    #[test]
    fn policy_keeps_allocation_bytes_for_partial_copies() {
        let mut v = payload();
        v["buffers"][0] = vec![0u8; 16].into();
        let b: Batch = serde_json::from_value(v).unwrap();
        assert!(b.validate().is_ok());
        let p = b.policy_inputs();
        assert_eq!(p[0]["bytes"], 20);
        assert_eq!(p[0]["sourceBytes"], 16);
        assert_eq!(p[0]["destinationBytes"], 4);
        assert_eq!(p[0]["copiedBytes"], 4);
        let mut v = payload();
        v["buffers"][1] = vec![0u8; 2 * 1024 * 1024].into();
        let b: Batch = serde_json::from_value(v).unwrap();
        assert!(b.validate().is_ok());
        let p = b.policy_inputs();
        assert_eq!(p[0]["bytes"], 2 * 1024 * 1024 + 4);
        assert_eq!(p[0]["destinationBytes"], 2 * 1024 * 1024);
    }
}
