use std::collections::BTreeMap;

use poem::{listener::TcpListener, Route};
use poem_openapi::{
    payload::Json, ApiResponse, Object, OpenApi, OpenApiService,
};
use slab::Slab;
use tokio::sync::Mutex;
use uuid::Uuid;

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
}

#[OpenApi]
impl Api {
    #[oai(path = "/jobs", method = "post")]
    async fn create_job(&self, job: Json<Job>) -> CreateJobResponse {
        let mut job = job.0;
        job.id = Uuid::new_v4().to_string();
        job.status = String::from("RUNNING");

        // Save jobs into slab. It is a demo so if use RDB is too big.
        // It can also changed to save a record in RDB such as MySQL.  
        let mut jobs = self.jobs.lock().await;
        let idx = jobs.insert(job.clone()) as i64;

        let res = redis_add_task(&job, "todo-list", idx).await;

        match res {
            Ok(_) => CreateJobResponse::Ok(Json(job)),
            Err(_) => CreateJobResponse::InternalError,
        }
    }

    #[oai(path = "/jobs/:job_id", method = "get")]
    async fn index(
        &self,
        #[oai(name = "job_id", in = "path")] job_id: String,
    ) -> FindJobResponse {
        match redis_hset_query(&job_id).await {
            Ok((job, slab_idx)) => FindJobResponse::Ok(Json(job.clone())),
            Err(_) => FindJobResponse::NotFound,
        }
    }

    /// Delete job by id
    #[oai(path = "/jobs/:job_id", method = "delete")]
    async fn delete_job(
        &self,
        #[oai(name = "job_id", in = "path")] job_id: String,
    ) -> DeleteJobResponse {
        match redis_hset_query(&job_id).await {
            Ok((_, slab_idx)) => {
                let mut jobs = self.jobs.lock().await;
                if jobs.contains(slab_idx as usize) {
                    // remove from slab
                    jobs.remove(slab_idx as usize);

                    // update cache
                    let _ = redis_delete(job_id, "running-list").await;
                    DeleteJobResponse::Ok
                } else {
                    DeleteJobResponse::NotFound
                }
            },
            Err(_) => DeleteJobResponse::NotFound,
        }


        // match self.index(job_id.clone()).await {
        //     FindJobResponse::Ok(_) => {
        //         let _ = redis_delete(job_id, "running-list").await;
        //         DeleteJobResponse::Ok
        //     },
        //     FindJobResponse::NotFound => DeleteJobResponse::NotFound,
        // }
    }

    /// Update job by id
    #[oai(path = "/jobs/:job_id", method = "put")]
    async fn put_job(
        &self,
        #[oai(name = "job_id", in = "path")] job_id: String,
        update: Json<UpdateJob>,
    ) -> UpdateJobResponse {
        match redis_hset_query(&job_id).await {
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
                        let _ = redis_update(&job, "running-list", slab_idx).await;
                        UpdateJobResponse::Ok
                    },
                    None => UpdateJobResponse::NotFound,
                }
                // } else {
                //     UpdateJobResponse::NotFound
                // }
            },
            Err(_) => UpdateJobResponse::NotFound,
        }
    }
}

async fn redis_hset_query(job_id: &str) -> redis::RedisResult<(Job, i64)> {
    let client = redis::Client::open("redis://redis/").unwrap();
    let mut con = client.get_async_connection().await?;

    let res: BTreeMap<String, String> = redis::cmd("HGETALL")
        .arg(job_id)
        .query_async(&mut con)
        .await?;
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

async fn redis_add_task(job: &Job, list: &str, idx: i64) -> redis::RedisResult<()> {
    let client = redis::Client::open("redis://redis/").unwrap();
    let mut con = client.get_async_connection().await?;

    let new_job = format!(
        "{}::{}::{}::{}::{}",
        job.id, job.content, job.schedule_type, job.duration, idx
    );
    let _ = redis::cmd("RPUSH")
        .arg(&[list, &new_job])
        .query_async(&mut con)
        .await?;

    Ok(())
}

async fn redis_update(job: &Job, list: &str, idx: i64) -> redis::RedisResult<()> {
    let client = redis::Client::open("redis://redis/").unwrap();
    let mut con = client.get_async_connection().await?;

    let new_job = format!(
        "{}::{}::{}::{}::{}",
        job.id, job.content, job.schedule_type, job.duration, idx
    );
    let update_job = format!("{}|update|{}", job.id, new_job);
    let _ = redis::cmd("RPUSH")
        .arg(&[list, &update_job])
        .query_async(&mut con)
        .await?;

    Ok(())
}

async fn redis_delete(id: String, list: &str) -> redis::RedisResult<()> {
    let client = redis::Client::open("redis://redis/").unwrap();
    let mut con = client.get_async_connection().await?;

    let delete_job = format!("{}|delete", id);
    let _ = redis::cmd("RPUSH")
        .arg(&[list, &delete_job])
        .query_async(&mut con)
        .await?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "poem=debug");
    }

    tracing_subscriber::fmt::init();

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
}