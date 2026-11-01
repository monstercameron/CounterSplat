//! Optional one-shot GPU timing for a range of encoded work.
//!
//! Timestamp queries measure GPU command execution between pass boundaries.
//! They do not include earlier queue writes, later presentation, or CPU time.
use std::sync::{Arc, Mutex};

const MAX_SPANS: usize = 8;
const QUERY_COUNT: u32 = MAX_SPANS as u32 + 1;
const BUFFER_BYTES: u64 = QUERY_COUNT as u64 * 8;

/// Nonblocking state of a one-shot GPU capture.
#[derive(Clone, Debug, PartialEq)]
pub enum GpuTiming {
    Ready,
    Pending {
        token: u64,
    },
    Complete {
        token: u64,
        milliseconds: f64,
        spans: Vec<(&'static str, f64)>,
    },
    Failed {
        token: u64,
        reason: String,
    },
}

#[derive(Clone, Debug)]
enum State {
    Ready,
    Encoding {
        token: u64,
        labels: Vec<&'static str>,
    },
    Encoded {
        token: u64,
        labels: Vec<&'static str>,
    },
    Pending(u64),
    Complete {
        token: u64,
        milliseconds: f64,
        spans: Vec<(&'static str, f64)>,
    },
    Failed(u64, String),
}

/// A single-use timer. Request `TIMESTAMP_QUERY` when creating the device;
/// `new` returns `None` on a device without that optional feature.
/// Encode `begin`, every `mark`, and `end` into the same command encoder,
/// submit that encoder to the queue, then call `submitted` on that queue.
pub struct GpuFrameTimer {
    queries: wgpu::QuerySet,
    resolved: wgpu::Buffer,
    readback: wgpu::Buffer,
    state: Arc<Mutex<State>>,
}

impl GpuFrameTimer {
    pub fn new(device: &wgpu::Device) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("frame timing queries"),
            ty: wgpu::QueryType::Timestamp,
            count: QUERY_COUNT,
        });
        let buffer = |label, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: BUFFER_BYTES,
                usage,
                mapped_at_creation: false,
            })
        };
        Some(Self {
            queries,
            resolved: buffer(
                "frame timing resolve",
                wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            ),
            readback: buffer(
                "frame timing readback",
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            ),
            state: Arc::new(Mutex::new(State::Ready)),
        })
    }

    fn timestamp(&self, encoder: &mut wgpu::CommandEncoder, index: u32) {
        let _pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("frame timing boundary"),
            timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                query_set: &self.queries,
                beginning_of_pass_write_index: Some(index),
                end_of_pass_write_index: None,
            }),
        });
    }

    /// Place the first timestamp. Returns false if the timer was already used.
    pub fn begin(&self, encoder: &mut wgpu::CommandEncoder, token: u64) -> bool {
        {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if !matches!(&*state, State::Ready) {
                return false;
            }
            *state = State::Encoding {
                token,
                labels: Vec::with_capacity(MAX_SPANS),
            };
        }
        self.timestamp(encoder, 0);
        true
    }

    /// Finish a named span and begin the next one. One slot remains for `end`.
    pub fn mark(&self, encoder: &mut wgpu::CommandEncoder, label: &'static str) -> bool {
        let index = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let State::Encoding { labels, .. } = &mut *state else {
                return false;
            };
            if labels.len() >= MAX_SPANS - 1 {
                return false;
            }
            labels.push(label);
            labels.len() as u32
        };
        self.timestamp(encoder, index);
        true
    }

    /// Finish the last named span and encode the query readback.
    pub fn end(&self, encoder: &mut wgpu::CommandEncoder, label: &'static str) -> bool {
        let count = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let State::Encoding { token, labels } = &mut *state else {
                return false;
            };
            labels.push(label);
            let count = labels.len() as u32 + 1;
            let token = *token;
            let labels = std::mem::take(labels);
            *state = State::Encoded { token, labels };
            count
        };
        self.timestamp(encoder, count - 1);
        encoder.resolve_query_set(&self.queries, 0..count, &self.resolved, 0);
        encoder.copy_buffer_to_buffer(&self.resolved, 0, &self.readback, 0, u64::from(count) * 8);
        true
    }

    /// Call after submitting the encoder. Mapping completes asynchronously.
    pub fn submitted(&self, queue: &wgpu::Queue) -> bool {
        let (token, labels) = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let State::Encoded { token, labels } = &*state else {
                return false;
            };
            let token = *token;
            let labels = labels.clone();
            *state = State::Pending(token);
            (token, labels)
        };
        let bytes = (labels.len() as u64 + 1) * 8;
        let period = queue.get_timestamp_period();
        let state = Arc::clone(&self.state);
        let buffer = self.readback.clone();
        self.readback
            .slice(0..bytes)
            .map_async(wgpu::MapMode::Read, move |result| {
                let mapped = result.is_ok();
                let measurement = result.map_err(|error| error.to_string()).and_then(|()| {
                    let data = buffer
                        .slice(0..bytes)
                        .get_mapped_range()
                        .map_err(|error| error.to_string())?;
                    let timestamps: Vec<_> = data
                        .chunks_exact(8)
                        .map(|chunk| {
                            u64::from_le_bytes(chunk.try_into().expect("timestamp length"))
                        })
                        .collect();
                    let milliseconds =
                        elapsed_ms(timestamps[0], timestamps[timestamps.len() - 1], period)?;
                    let spans = labels
                        .iter()
                        .zip(timestamps.windows(2))
                        .map(|(&name, pair)| {
                            elapsed_ms(pair[0], pair[1], period).map(|value| (name, value))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok((milliseconds, spans))
                });
                if mapped {
                    buffer.unmap();
                }
                let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
                if matches!(&*state, State::Pending(current) if *current == token) {
                    *state = match measurement {
                        Ok((milliseconds, spans)) => State::Complete {
                            token,
                            milliseconds,
                            spans,
                        },
                        Err(reason) => State::Failed(token, reason),
                    };
                }
            });
        true
    }

    /// Observe the nonblocking state of this one-shot capture.
    pub fn timing(&self) -> GpuTiming {
        match &*self.state.lock().unwrap_or_else(|error| error.into_inner()) {
            State::Ready => GpuTiming::Ready,
            State::Encoding { token, .. }
            | State::Encoded { token, .. }
            | State::Pending(token) => GpuTiming::Pending { token: *token },
            State::Complete {
                token,
                milliseconds,
                spans,
            } => GpuTiming::Complete {
                token: *token,
                milliseconds: *milliseconds,
                spans: spans.clone(),
            },
            State::Failed(token, reason) => GpuTiming::Failed {
                token: *token,
                reason: reason.clone(),
            },
        }
    }
}

fn elapsed_ms(start: u64, end: u64, period: f32) -> Result<f64, String> {
    if !period.is_finite() || period <= 0.0 {
        return Err("invalid GPU timestamp period".into());
    }
    let ticks = end
        .checked_sub(start)
        .ok_or_else(|| "GPU timestamps are out of order".to_owned())?;
    Ok(ticks as f64 * f64::from(period) / 1_000_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_conversion_checks_invalid_values() {
        assert_eq!(elapsed_ms(10, 110, 2.0).unwrap(), 0.0002);
        assert!(elapsed_ms(110, 10, 1.0).is_err());
        assert!(elapsed_ms(0, 1, f32::NAN).is_err());
        assert!(elapsed_ms(0, 1, 0.0).is_err());
    }

    #[test]
    fn unsupported_device_does_not_create_a_timer() {
        if let Some((device, _)) = crate::try_device() {
            if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
                assert!(GpuFrameTimer::new(&device).is_none());
            }
        }
    }

    #[test]
    fn measures_submitted_gpu_work_when_timestamps_are_supported() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Ok(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        else {
            return;
        };
        if !adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return;
        }
        let Ok((device, queue)) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                required_features: wgpu::Features::TIMESTAMP_QUERY,
                ..Default::default()
            }))
        else {
            return;
        };
        let timer = GpuFrameTimer::new(&device).unwrap();
        let mut encoder = device.create_command_encoder(&Default::default());
        assert!(!timer.mark(&mut encoder, "early"));
        assert!(!timer.end(&mut encoder, "early"));
        assert!(!timer.submitted(&queue));
        assert!(timer.begin(&mut encoder, 42));
        assert!(!timer.begin(&mut encoder, 43));
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 1024 * 1024,
            usage: wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.clear_buffer(&buffer, 0, None);
        assert!(timer.mark(&mut encoder, "clear"));
        assert!(timer.end(&mut encoder, "finish"));
        assert!(!timer.end(&mut encoder, "late"));
        queue.submit(Some(encoder.finish()));
        assert!(timer.submitted(&queue));
        assert!(!timer.submitted(&queue));
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        match timer.timing() {
            GpuTiming::Complete {
                token,
                milliseconds,
                spans,
            } => {
                assert_eq!(token, 42);
                assert_eq!(
                    spans.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
                    ["clear", "finish"]
                );
                assert!(milliseconds.is_finite() && milliseconds >= 0.0);
                assert!(spans
                    .iter()
                    .all(|(_, value)| value.is_finite() && *value >= 0.0));
                assert!(
                    (spans.iter().map(|(_, value)| value).sum::<f64>() - milliseconds).abs() < 1e-9
                );
                println!("GPU timestamp smoke test: {milliseconds:.6} ms");
            }
            other => panic!("unexpected timing state: {other:?}"),
        }

        let bounded = GpuFrameTimer::new(&device).unwrap();
        let mut encoder = device.create_command_encoder(&Default::default());
        assert!(bounded.begin(&mut encoder, 43));
        for _ in 0..MAX_SPANS - 1 {
            assert!(bounded.mark(&mut encoder, "intermediate"));
        }
        assert!(!bounded.mark(&mut encoder, "too many"));
        assert!(bounded.end(&mut encoder, "final"));
        queue.submit(Some(encoder.finish()));
        assert!(bounded.submitted(&queue));
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        match bounded.timing() {
            GpuTiming::Complete { token, spans, .. } => {
                assert_eq!(token, 43);
                assert_eq!(spans.len(), MAX_SPANS);
                assert_eq!(spans.last().unwrap().0, "final");
            }
            other => panic!("unexpected timing state: {other:?}"),
        }
    }
}
