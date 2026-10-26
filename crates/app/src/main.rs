#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    let demo = std::env::args().any(|a| a == "--demo");
    if demo {
        let _backend = ytm_core::FixtureBackend::new();
    }
    todo!("egui shell")
}
