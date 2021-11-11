use std::collections::BTreeMap;
use std::io::Error;

use poem::{listener::TcpListener, Route};
use poem_openapi::{payload::Json, ApiResponse, Object, OpenApi, OpenApiService};
use slab::Slab;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use uuid::Uuid;

use mobc_redis::mobc;
use mobc_redis::redis;
use mobc_redis::redis::AsyncCommands;
use mobc_redis::RedisConnectionManager;

pub type Connection = mobc::Connection<RedisConnectionManager>;

#[derive(Clone)]
pub struct RedisPool {
    pub pool: mobc::Pool<RedisConnectionManager>,
}

impl Default for RedisPool {
    fn default() -> Self {
        let redis = redis::Client::open("redis://127.0.0.1").unwrap();
        let mgr = RedisConnectionManager::new(redis);
        let pool = mobc::Pool::builder().max_open(20).build(mgr);
        Self { pool }
    }
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

pub async fn http_server_start() -> JoinHandle<Result<(), Error>> {
    tokio::spawn(async move {
        // Create a TCP listener
        let listener = TcpListener::bind("127.0.0.1:3000");

        // Create API service
        let api_service = OpenApiService::new(Api::default())
            .title("Hello World")
            .server("http://localhost:3000/api");

        // Enable the Swagger UI
        let ui = api_service.swagger_ui();

        // Start the server and specify that the root path of the API is /api, and the path of Swagger UI is /
        poem::Server::new(listener)
            .await?
            .run(Route::new().nest("/api", api_service).nest("/", ui))
            .await
    })
}

#[derive(Debug, Object, Clone, Eq, PartialEq)]
struct Job {
    #[oai(read_only)]
    id: String,

    #[oai(max_length = 64)]
    content: String,

    schedule_type: String,

    duration: i64,

    #[oai(read_only)]
    status: String,
}

#[derive(Debug, Object, Clone, Eq, PartialEq)]
struct UpdateJob {
    content: Option<String>,
    schedule_type: Option<String>,
    duration: Option<i64>,
}

#[derive(ApiResponse)]
enum CreateJobResponse {
    /// Returns when the job is successfully created.
    #[oai(status = 200)]
    Ok(Json<Job>),
    #[oai(status = 500)]
    InternalError,
}

#[derive(ApiResponse)]
enum FindJobResponse {
    /// Return the specified job.
    #[oai(status = 200)]
    Ok(Json<Job>),
    /// Return when the specified job is not found.
    #[oai(status = 404)]
    NotFound,
}

#[derive(ApiResponse)]
enum DeleteJobResponse {
    /// Returns when the job is successfully deleted.
    #[oai(status = 200)]
    Ok,
    /// Return when the specified job is not found.
    #[oai(status = 404)]
    NotFound,
}

#[derive(ApiResponse)]
enum UpdateJobResponse {
    /// Returns when the job is successfully updated.
    #[oai(status = 200)]
    Ok,
    /// Return when the specified job is not found.
    #[oai(status = 404)]
    NotFound,
}

#[derive(Default)]
struct Api {
    jobs: Mutex<Slab<Job>>,
    redis_pool: RedisPool,
}

#[OpenApi]
impl Api {
    // 创建任务：
    // 接口接收由 3 个参数组成的 Json 格式对象：
    // content【 任务内容 】
    // schedule_type【 任务类型：可重复执行：Repeated / 仅一次：OneShot】
    // duration【 可重复执行任务时，此参数为循环的时间间隔，仅一次的任务时，此参数为多久后执行 】
    // 组装数据，为任务生成 id【 uuid 】，以及初始运行状态【 status：RUNNING 】，加上提交的三个参数生成 Job 对象。
    // 存储：将对象参数存入 slab 中，这个流程用于模拟数据库操作。
    // 此处因为是 demo，未使用 RDB 这类重型的组件，例如 MySQL，
    // 后期改造为使用 RDB 如 MySQL 后可以在 Scheduler 中新建一个任务，定时从数据库中查询应执行的任务，
    // 并且在 Redis 中进行检查，如果不存在就可以进行启动。
    // 缓存：将存入 slab 返回的 key 以及 Job 对象调用 redis_add_task 方法，写入 Redis 中的 Todo-List 中。
    #[oai(path = "/jobs", method = "post")]
    async fn create_job(&self, job: Json<Job>) -> CreateJobResponse {
        let mut job = job.0;
        job.id = Uuid::new_v4().to_string();
        job.status = String::from("RUNNING");

        // Save jobs into slab. It is a demo so if use RDB is too big.
        // It can also changed to save a record in RDB such as MySQL.
        let mut jobs = self.jobs.lock().await;
        let idx = jobs.insert(job.clone()) as i64;

        let res = self.redis_add_task(&job, "todo-list", idx).await;

        match res {
            Ok(_) => CreateJobResponse::Ok(Json(job)),
            Err(_) => CreateJobResponse::InternalError,
        }
    }

    // 查询任务：
    // 接口接收 1 个参数：任务ID【 uuid 】
    // 通过 任务ID 到 Redis 中的 HSET 查找，如果找到返回任务信息，否则返回 404.
    #[oai(path = "/jobs/:job_id", method = "get")]
    async fn index(&self, #[oai(name = "job_id", in = "path")] job_id: String) -> FindJobResponse {
        match self.redis_hset_query(&job_id).await {
            Ok((job, slab_idx)) => FindJobResponse::Ok(Json(job.clone())),
            Err(_) => FindJobResponse::NotFound,
        }
    }

    // 删除任务：
    // 接口接收 1 个参数：任务ID【 uuid 】
    // 通过 任务ID 到 Redis 中的 HSET 查找，
    // 如果找到，就把从 hset 中拿到 slab_idx，根据 slab_id 移除元素，用于模拟数据库的删除操作【物理删除或者逻辑删除】
    // slab 处理完成后，将数据按照 job_id|delete 的格式写入 Running-List
    // 否则返回 404.
    #[oai(path = "/jobs/:job_id", method = "delete")]
    async fn delete_job(
        &self,
        #[oai(name = "job_id", in = "path")] job_id: String,
    ) -> DeleteJobResponse {
        match self.redis_hset_query(&job_id).await {
            Ok((_, slab_idx)) => {
                let mut jobs = self.jobs.lock().await;
                if jobs.contains(slab_idx as usize) {
                    // remove from slab
                    jobs.remove(slab_idx as usize);

                    // update cache
                    let _ = self.redis_delete(job_id, "running-list").await;
                    DeleteJobResponse::Ok
                } else {
                    DeleteJobResponse::NotFound
                }
            }
            Err(_) => DeleteJobResponse::NotFound,
        }
    }

    // 修改任务：
    // 接口接收 1 个参数：任务ID【 uuid 】，和一个 PUT 传递过来的包含任务内容，任务类型，任务时间三个参数组成的 UpdateJob 对象。
    // 通过 任务ID 到 Redis 中的 HSET 查找，
    // 如果找到，就把从 hset 中拿到 slab_idx，根据 slab_id 从更新元素，用于模拟数据库的更新操作
    // slab 处理完成后，将数据按照 job_id|update|id::content::schedule_type::duration::idx 的格式写入 Running-List
    // 否则返回 404.
    #[oai(path = "/jobs/:job_id", method = "put")]
    async fn put_job(
        &self,
        #[oai(name = "job_id", in = "path")] job_id: String,
        update: Json<UpdateJob>,
    ) -> UpdateJobResponse {
        match self.redis_hset_query(&job_id).await {
            Ok((job, slab_idx)) => {
                let mut jobs = self.jobs.lock().await;
                match jobs.get_mut(slab_idx as usize) {
                    Some(slab_job) => {
                        let mut job = job;
                        if let Some(content) = update.0.content {
                            job.content = content.clone();
                            slab_job.content = content;
                        }
                        if let Some(schedule_type) = update.0.schedule_type {
                            job.schedule_type = schedule_type.clone();
                            slab_job.schedule_type = schedule_type;
                        }
                        if let Some(duration) = update.0.duration {
                            job.duration = duration;
                            slab_job.duration = duration;
                        }
                        let _ = self.redis_update(&job, "running-list", slab_idx).await;
                        UpdateJobResponse::Ok
                    }
                    None => UpdateJobResponse::NotFound,
                }
            }
            Err(_) => UpdateJobResponse::NotFound,
        }
    }

    async fn redis_add_task(&self, job: &Job, list: &str, idx: i64) -> redis::RedisResult<()> {
        let new_job = format!(
            "{}::{}::{}::{}::{}",
            job.id, job.content, job.schedule_type, job.duration, idx
        );
        let mut con = self.redis_pool.clone().get_connection().await.unwrap();
        let _ = con.rpush(list, new_job).await?;
        Ok(())
    }

    async fn redis_hset_query(&self, job_id: &str) -> redis::RedisResult<(Job, i64)> {
        let mut con = self.redis_pool.clone().get_connection().await.unwrap();
        let cache_query = con.hgetall(job_id).await;
        match cache_query {
            Ok(res) => {
                let res: BTreeMap<String, String> = res;
                let job: Job = Job {
                    id: res.get("id").unwrap().clone(),
                    content: res.get("content").unwrap().clone(),
                    schedule_type: res.get("schedule_type").unwrap().clone(),
                    duration: res.get("duration").unwrap().parse::<i64>().unwrap(),
                    status: res.get("status").unwrap().clone(),
                };
                let slab_idx = res.get("slab_idx").unwrap().parse::<i64>().unwrap();
                Ok((job, slab_idx))
            }
            Err(error) => Err(error),
        }
    }

    async fn redis_update(&self, job: &Job, list: &str, idx: i64) -> redis::RedisResult<()> {
        let mut con = self.redis_pool.clone().get_connection().await.unwrap();

        let new_job = format!(
            "{}::{}::{}::{}::{}",
            job.id, job.content, job.schedule_type, job.duration, idx
        );
        let update_job = format!("{}|update|{}", job.id, new_job);
        let _ = con.rpush(list, update_job).await?;

        Ok(())
    }

    async fn redis_delete(&self, id: String, list: &str) -> redis::RedisResult<()> {
        let mut con = self.redis_pool.clone().get_connection().await.unwrap();
        let delete_job = format!("{}|delete", id);
        let _ = con.rpush(list, delete_job).await?;

        Ok(())
    }
}
