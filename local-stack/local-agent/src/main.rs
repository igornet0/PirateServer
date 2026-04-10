//! Workstation-side agent placeholder (tunnel / outbound connection — see `docs/LOCAL_STACK_DESIGN.md`).

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "local-agent",
    version,
    about = "Stub for future PC ↔ server connectivity (see docs/LOCAL_STACK_DESIGN.md)"
)]
struct Cli {}

fn main() {
    let _ = Cli::parse();
    println!("local-agent: stub — see docs/LOCAL_STACK_DESIGN.md");
}
