use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    match env::args().nth(1).as_deref() {
        Some("build") => build(),
        Some("run") => run(),
        _ => {
            eprintln!("usage: cargo xtask <build|run>");
            ExitCode::from(2)
        }
    }
}

fn build() -> ExitCode {
    if !command("cargo", &["test", "-p", "lumora-kernel"]) {
        return ExitCode::from(1);
    }
    if !command(
        "cargo",
        &[
            "build",
            "-p",
            "lumora-boot",
            "--target",
            "x86_64-unknown-uefi",
        ],
    ) {
        return ExitCode::from(1);
    }
    let out = PathBuf::from("target/lumora-esp/EFI/BOOT");
    if let Err(error) = fs::create_dir_all(&out) {
        eprintln!("failed to create ESP directory: {error}");
        return ExitCode::from(1);
    }
    if let Err(error) = fs::copy(
        "target/x86_64-unknown-uefi/debug/lumora-boot.efi",
        out.join("BOOTX64.EFI"),
    ) {
        eprintln!("failed to stage bootloader: {error}");
        return ExitCode::from(1);
    }
    println!("Lumora build: staged UEFI image at target/lumora-esp");
    ExitCode::SUCCESS
}

fn run() -> ExitCode {
    if build() != ExitCode::SUCCESS {
        return ExitCode::from(1);
    }
    let vars = PathBuf::from("target/OVMF_VARS_4M.fd");
    if let Err(error) = fs::copy("/usr/share/OVMF/OVMF_VARS_4M.fd", &vars) {
        eprintln!("failed to copy writable OVMF variables: {error}");
        return ExitCode::from(1);
    }
    if !command(
        "qemu-system-x86_64",
        &[
            "-machine",
            "q35",
            "-drive",
            "if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd",
            "-drive",
            "if=pflash,format=raw,file=target/OVMF_VARS_4M.fd",
            "-drive",
            "format=raw,file=fat:rw:target/lumora-esp",
            "-nographic",
            "-serial",
            "stdio",
            "-monitor",
            "none",
        ],
    ) {
        eprintln!("QEMU/OVMF is required for cargo xtask run.");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn command(program: &str, arguments: &[&str]) -> bool {
    match Command::new(program).args(arguments).status() {
        Ok(status) => status.success(),
        Err(error) => {
            eprintln!("failed to run {program}: {error}");
            false
        }
    }
}
