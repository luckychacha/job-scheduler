
use mobc_redis::mobc;
use mobc_redis::redis;
use mobc_redis::RedisConnectionManager;

pub type Connection = mobc::Connection<RedisConnectionManager>;

pub struct RedisModule {
    pool: mobc::Pool<RedisConnectionManager>,
}

impl RedisModule {
    pub fn new() -> Option<Self> {
        match redis::Client::open("redis://127.0.0.1/") {
            Ok(it) => {
                let mgr = RedisConnectionManager::new(it);
                let pool = mobc::Pool::builder().max_open(20).build(mgr);
                Some(Self { pool })
            },
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