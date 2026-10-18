use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let exception_test = arguments
        .iter()
        .any(|argument| argument == "exception-test");
    match arguments.first().map(String::as_str) {
        Some("build") => build(exception_test),
        Some("run") => run(exception_test),
        _ => {
            eprintln!("usage: cargo xtask <build|run> [exception-test]");
            ExitCode::from(2)
        }
    }
}

fn build_hello() -> bool {
    let mut script = PathBuf::from("apps");
    script.push("hello");
    script.push("link.ld");
    let rustflags = format!(
        "-C link-arg=-T{} -C relocation-model=static -C code-model=large",
        script.display()
    );
    match Command::new("cargo")
        .args(["build", "-p", "hello", "--target", "x86_64-unknown-none"])
        .env("RUSTFLAGS", rustflags)
        .status()
    {
        Ok(status) => {
            if !status.success() {
                eprintln!("hello user program failed to build");
            }
            status.success()
        }
        Err(error) => {
            eprintln!("failed to build hello: {error}");
            false
        }
    }
}

fn build(exception_test: bool) -> ExitCode {
    if !build_hello() {
        return ExitCode::from(1);
    }
    if !command("cargo", &["test", "-p", "lumora-kernel"]) {
        return ExitCode::from(1);
    }
    let mut boot = vec![
        "build",
        "-p",
        "lumora-boot",
        "--target",
        "x86_64-unknown-uefi",
    ];
    if exception_test {
        boot.extend(["--features", "lumora-boot/exception-test"]);
    }
    if !command("cargo", &boot) {
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
    if exception_test {
        println!("Lumora build: exception-test feature enabled");
    }
    println!("Lumora build: staged UEFI image at target/lumora-esp");
    ExitCode::SUCCESS
}

fn run(exception_test: bool) -> ExitCode {
    if build(exception_test) != ExitCode::SUCCESS {
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
