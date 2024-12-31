use std::{env, path::Path, process::Command};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let task = if let Some(task) = std::env::args().nth(1) {
        task
    } else {
        "build".to_string()
    };

    match task.as_str() {
        "build" => command("build"),
        "run" => command("run"),
        _ => return Err(anyhow::anyhow! { "Unknown command" }.into()),
    }?;

    Ok(())
}

fn command(input: &str) -> Result<(), Box<dyn std::error::Error>> {
    let cargo: String = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let status = Command::new(cargo)
        .current_dir(
            Path::new(&env!("CARGO_MANIFEST_DIR"))
                .ancestors()
                .nth(1)
                .unwrap()
                .to_path_buf(),
        )
        .args([input, "--release", "--example", "simple"])
        .status()?;

    if !status.success() {
        return Err(anyhow::anyhow! { "example run step failed" }.into());
    }
    Ok(())
}
