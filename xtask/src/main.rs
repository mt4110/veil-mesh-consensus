use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Parser)]
#[command(
    name = "xtask",
    about = "veil-mesh-consensus build & eBPF orchestration tool"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 環境変数 (`KERNEL_HEADERS`, `VMLINUX_PATH`) と開発ツールチェーンの診断
    Doctor,
    /// `veil-ebpf` を `bpfel-unknown-none` ターゲットでビルド
    BuildEbpf {
        /// Release ビルドを行うかどうか
        #[arg(long)]
        release: bool,
    },
    /// `aya-tool` を用いて `VMLINUX_PATH` から Rust BTF バインディングを生成
    GenVmlinux {
        /// 抽出するカーネル構造体名（スペース区切り）
        #[arg(default_values_t = ["ethhdr".to_string(), "iphdr".to_string(), "sockaddr_in".to_string()])]
        types: Vec<String>,
    },
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask must live inside workspace root")
        .to_path_buf()
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Doctor => run_doctor(),
        Commands::BuildEbpf { release } => build_ebpf(release),
        Commands::GenVmlinux { types } => gen_vmlinux(&types),
    }
}

fn run_doctor() -> Result<()> {
    println!("=== veil-mesh-consensus Environment Doctor ===");
    for var in ["KERNEL_HEADERS", "VMLINUX_PATH", "C_INCLUDE_PATH"] {
        match env::var(var) {
            Ok(val) => {
                let exists = Path::new(&val.split(':').next().unwrap_or("")).exists();
                println!("  [OK] {var} = {val} (exists: {exists})");
            }
            Err(_) => {
                println!(
                    "  [WARN] {var} is not set (run inside `nix develop` or enable `direnv allow`)"
                );
            }
        }
    }
    Ok(())
}

fn build_ebpf(release: bool) -> Result<()> {
    let root = workspace_root();
    let ebpf_dir = root.join("veil-ebpf");

    if let Ok(headers) = env::var("KERNEL_HEADERS") {
        println!("Using KERNEL_HEADERS={headers}");
    }

    let mut cmd = Command::new("cargo");
    cmd.current_dir(&ebpf_dir).args([
        "build",
        "--target=bpfel-unknown-none",
        "-Z",
        "build-std=core",
    ]);

    if release {
        cmd.arg("--release");
    }

    let status = cmd
        .status()
        .context("Failed to execute cargo build for veil-ebpf")?;

    if !status.success() {
        bail!("veil-ebpf build failed");
    }

    println!("Successfully built veil-ebpf!");
    Ok(())
}

fn gen_vmlinux(types: &[String]) -> Result<()> {
    let vmlinux = env::var("VMLINUX_PATH")
        .context("VMLINUX_PATH is not set. Run inside `nix develop` or `direnv`")?;

    let root = workspace_root();
    let out_file = root.join("veil-ebpf/src/vmlinux.rs");

    let mut cmd = Command::new("aya-tool");
    cmd.arg("generate").arg("--btf").arg(&vmlinux).args(types);

    let output = cmd.output().context("Failed to run `aya-tool generate`")?;
    if !output.status.success() {
        bail!(
            "aya-tool failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    std::fs::write(&out_file, &output.stdout)
        .with_context(|| format!("Failed to write {}", out_file.display()))?;

    println!("Generated BTF bindings at {}", out_file.display());
    Ok(())
}
