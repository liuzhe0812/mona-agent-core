fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(code) = server::dispatch_helper() {
        std::process::exit(code);
    }
    serve()
}

#[tokio::main]
async fn serve() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    server::serve_until(std::future::pending()).await
}
