#[cfg(target_os = "linux")]
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    veil_node::local_ban::runtime::run().await
}

#[cfg(not(target_os = "linux"))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("veil-local-ban requires Linux; the portable policy tests run on other hosts")
}
