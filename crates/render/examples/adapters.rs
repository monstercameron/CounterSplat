//! Lists every GPU adapter wgpu can see, with the limits the renderer cares about.
//!
//! ```text
//! cargo run --release -p countersplat-render --example adapters
//! ```
//!
//! Select one for the tests and benchmarks with `WGPU_ADAPTER_NAME=<part of the name>` (and `WGPU_BACKEND`).
fn main() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    if adapters.is_empty() {
        println!("no adapters");
    }
    for adapter in adapters {
        let info = adapter.get_info();
        let limits = adapter.limits();
        println!(
            "{:?} | {} | {:?} | vendor {:#06x} device {:#06x} | driver {} {}",
            info.backend,
            info.name,
            info.device_type,
            info.vendor,
            info.device,
            info.driver,
            info.driver_info
        );
        println!(
            "    max storage binding {} MiB, max buffer {} MiB, max texture 2d {}, timestamps {}",
            limits.max_storage_buffer_binding_size >> 20,
            limits.max_buffer_size >> 20,
            limits.max_texture_dimension_2d,
            adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY)
        );
    }
}
