//! First offscreen rendering slice: one RGBA8 target, one triangle-list draw.
//! This deliberately does not claim texture sampling or full WebGPU semantics.
use super::webgpu::{Session, checked, err};
use deno_error::JsErrorBox;
use serde::Deserialize;
use wgpu::util::DeviceExt;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RenderJob {
    pub width: u32,
    pub height: u32,
    pub vertex: String,
    pub fragment: String,
    pub vertex_entry: String,
    pub fragment_entry: String,
    pub clear: [f64; 4],
    pub draw: [u32; 4],
    pub layouts: Vec<Layout>,
    pub buffers: Vec<Vec<u8>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Layout {
    stride: u64,
    instance: bool,
    attributes: Vec<Attribute>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Attribute {
    offset: u64,
    location: u32,
    format: String,
}
fn format(s: &str) -> Result<wgpu::VertexFormat, JsErrorBox> {
    match s {
        "float32x2" => Ok(wgpu::VertexFormat::Float32x2),
        "float32x3" => Ok(wgpu::VertexFormat::Float32x3),
        "float32x4" => Ok(wgpu::VertexFormat::Float32x4),
        _ => Err(err("unsupported vertex format")),
    }
}
impl RenderJob {
    pub fn validate(&self) -> Result<(), JsErrorBox> {
        if self.width == 0 || self.height == 0 || self.width > 1024 || self.height > 1024 {
            return Err(err("render dimensions limit"));
        }
        if self.vertex.len() > 65536
            || self.fragment.len() > 65536
            || self.vertex_entry.len() > 256
            || self.fragment_entry.len() > 256
        {
            return Err(err("render shader limit"));
        }
        if self.clear.iter().any(|x| !x.is_finite()) {
            return Err(err("invalid clear color"));
        }
        if u64::from(self.draw[0]) * u64::from(self.draw[1]) > 65536
            || self.draw[2] != 0
            || self.draw[3] != 0
        {
            return Err(err("draw limit; first vertex/instance must be zero"));
        }
        if self.layouts.len() > 4
            || self.layouts.len() != self.buffers.len()
            || self.buffers.iter().map(Vec::len).sum::<usize>() > 65536
        {
            return Err(err("vertex buffer limit"));
        }
        for (l, b) in self.layouts.iter().zip(&self.buffers) {
            if l.stride == 0
                || l.stride > 2048
                || l.stride % 4 != 0
                || l.attributes.len() > 8
                || b.is_empty()
                || b.len() % 4 != 0
            {
                return Err(err("invalid vertex layout"));
            }
            for a in &l.attributes {
                let f = format(&a.format)?;
                if a.offset % 4 != 0
                    || a.offset > l.stride
                    || a.offset + f.size() > l.stride
                    || a.location >= 16
                {
                    return Err(err("invalid vertex attribute"));
                }
            }
        }
        Ok(())
    }
    pub fn bytes(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height) * 4
    }
}
pub(super) fn render(session: &Session, job: RenderJob) -> Result<String, JsErrorBox> {
    let _guard = session
        .busy
        .try_lock()
        .map_err(|_| err("device already dispatching"))?;
    let device = &session.device;
    let attributes: Vec<Vec<_>> = job
        .layouts
        .iter()
        .map(|l| {
            l.attributes
                .iter()
                .map(|a| {
                    Ok(wgpu::VertexAttribute {
                        format: format(&a.format)?,
                        offset: a.offset,
                        shader_location: a.location,
                    })
                })
                .collect::<Result<_, JsErrorBox>>()
        })
        .collect::<Result<_, _>>()?;
    let layouts: Vec<_> = job
        .layouts
        .iter()
        .zip(&attributes)
        .map(|(l, a)| wgpu::VertexBufferLayout {
            array_stride: l.stride,
            step_mode: if l.instance {
                wgpu::VertexStepMode::Instance
            } else {
                wgpu::VertexStepMode::Vertex
            },
            attributes: a,
        })
        .collect();
    let pipeline = checked(device, || {
        let vs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(job.vertex.clone().into()),
        });
        let fs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(job.fragment.clone().into()),
        });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: None,
            vertex: wgpu::VertexState {
                module: &vs,
                entry_point: Some(&job.vertex_entry),
                compilation_options: Default::default(),
                buffers: &layouts,
            },
            fragment: Some(wgpu::FragmentState {
                module: &fs,
                entry_point: Some(&job.fragment_entry),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        })
    })?;
    let pitch = (job.width * 4).div_ceil(256) * 256;
    let readback = checked(device, || {
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: job.width,
                height: job.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let buffers: Vec<_> = job
            .buffers
            .iter()
            .map(|b| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: b,
                    usage: wgpu::BufferUsages::VERTEX,
                })
            })
            .collect();
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(pitch) * u64::from(job.height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: job.clear[0],
                            g: job.clear[1],
                            b: job.clear[2],
                            a: job.clear[3],
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&pipeline);
            for (i, b) in buffers.iter().enumerate() {
                pass.set_vertex_buffer(i as u32, b.slice(..));
            }
            pass.draw(0..job.draw[0], 0..job.draw[1]);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(pitch),
                    rows_per_image: Some(job.height),
                },
            },
            wgpu::Extent3d {
                width: job.width,
                height: job.height,
                depth_or_array_layers: 1,
            },
        );
        session.queue.submit([encoder.finish()]);
        readback
    })?;
    let (tx, rx) = std::sync::mpsc::channel();
    readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        device.poll(wgpu::PollType::Poll).map_err(err)?;
        match rx.try_recv() {
            Ok(r) => {
                r.map_err(err)?;
                break;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                return Err(err("render map callback lost"));
            }
            Err(_) => {}
        }
        if std::time::Instant::now() >= deadline {
            device.destroy();
            return Err(err("render readback timeout"));
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let data = readback.slice(..).get_mapped_range();
    let mut packed = Vec::with_capacity(job.bytes() as usize);
    for row in data.chunks(pitch as usize) {
        packed.extend_from_slice(&row[..job.width as usize * 4]);
    }
    drop(data);
    readback.unmap();
    serde_json::to_string(&packed).map_err(err)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn render_bounds() {
        let mut j: RenderJob=serde_json::from_str(r#"{"width":64,"height":64,"vertex":"","fragment":"","vertex_entry":"vs","fragment_entry":"fs","clear":[0,0,0,1],"draw":[3,1,0,0],"layouts":[],"buffers":[]}"#).unwrap();
        assert!(j.validate().is_ok());
        j.width = u32::MAX;
        assert!(j.validate().is_err());
        j.width = 64;
        j.draw = [u32::MAX, u32::MAX, 0, 0];
        assert!(j.validate().is_err());
        j.draw = [3, 1, 0, 0];
        j.layouts = vec![Layout {
            stride: 8,
            instance: false,
            attributes: vec![Attribute {
                offset: u64::MAX,
                location: 0,
                format: "float32x2".into(),
            }],
        }];
        j.buffers = vec![vec![0; 8]];
        assert!(j.validate().is_err());
    }
}
