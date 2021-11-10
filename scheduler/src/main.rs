mod server;
mod deal_schedule;

async fn async_main()  {

    let jh = server::http_server_start().await;

    let _ = deal_schedule::schedule_start().await;

    let _ = jh.await;
}


#[tokio::main]
async fn main() {
    async_main().await;
}