mod server;
mod deal_schedule;
// mod redis_pool;

async fn async_main()  {

    // let mut conn = redis_pool::get_redis_pool().await;
    // let redis = redis_pool::RedisModule::new().unwrap();
    let jh = server::http_server_start().await;

    // let schedule_module = Scheduler::new().unwrap();
    let _ = deal_schedule::schedule_start().await;
    // let _ = schedule_module.schedule_start().await;

    let _ = jh.await;
}


#[tokio::main]
async fn main() {
    async_main().await;
}