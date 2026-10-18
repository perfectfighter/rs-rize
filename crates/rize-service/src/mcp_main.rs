#[tokio::main]
async fn main() {
    if let Err(error) = rize_service::mcp::run_stdio().await {
        eprintln!("{}", error.message);
        std::process::exit(1);
    }
}
