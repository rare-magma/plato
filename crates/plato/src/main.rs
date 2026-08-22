mod app;

use plato_core::anyhow::Error;
use crate::app::run;

fn main() -> Result<(), Error> {
    std::panic::set_hook(Box::new(|info| {
        eprintln!("[plato] PANIC: {}", info);
    }));

    if let Err(error) = run() {
        eprintln!("[plato] Fatal error: {:#}", error);
        return Err(error);
    }
    Ok(())
}
