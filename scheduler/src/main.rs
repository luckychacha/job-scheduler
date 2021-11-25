mod deal_schedule;
mod server;

use mobc_redis::redis;
use mobc_redis::mobc;
use mobc_redis::RedisConnectionManager;
use lazy_static::lazy_static;

lazy_static! {
    static ref REDIS_POOL: RedisPool = RedisPool::new().unwrap();
}

pub async fn connection() -> Connection {
    REDIS_POOL.get_connection().await.unwrap()
}

pub type Connection = mobc::Connection<RedisConnectionManager>;

#[derive(Clone)]
pub struct RedisPool {
    pub pool: mobc::Pool<RedisConnectionManager>,
}

impl RedisPool {
    pub fn new() -> Option<Self> {
        match redis::Client::open("redis://127.0.0.1") {
            Ok(it) => {
                let mgr = RedisConnectionManager::new(it);
                let pool = mobc::Pool::builder().max_open(20).build(mgr);
                Some(Self { pool })
            }
            Err(_) => return None,
        }
    }

    pub async fn get_connection(&self) -> Option<Connection> {
        match self.pool.get().await {
            Ok(it) => Some(it),
            Err(_) => return None,
        }
    }
}

async fn async_main() {
    lazy_static::initialize(&REDIS_POOL);

    let jh = server::http_server_start().await;

    let _ = deal_schedule::schedule_start().await;

    let _ = jh.await;
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    async_main().await;
}
