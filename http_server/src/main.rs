use std::collections::BTreeMap;

use chrono::{DateTime, Local};
use poem::{listener::TcpListener, Route};
use poem_openapi::{
    payload::Json, types::Password, ApiResponse, Object, OpenApi, OpenApiService, Tags,
};
use redis::{aio::ConnectionLike, AsyncCommands, Cmd};
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
    /// Returns when the user is successfully created.
    #[oai(status = 200)]
    Ok(Json<Job>),
    #[oai(status = 500)]
    InternalError,
}

#[derive(ApiResponse)]
enum FindJobResponse {
    /// Return the specified user.
    #[oai(status = 200)]
    Ok(Json<Job>),
    /// Return when the specified user is not found.
    #[oai(status = 404)]
    NotFound,
}

#[derive(ApiResponse)]
enum DeleteJobResponse {
    /// Returns when the user is successfully deleted.
    #[oai(status = 200)]
    Ok,
    /// Return when the specified user is not found.
    #[oai(status = 404)]
    NotFound,
}

#[derive(ApiResponse)]
enum UpdateJobResponse {
    /// Returns when the user is successfully updated.
    #[oai(status = 200)]
    Ok,
    /// Return when the specified user is not found.
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
    async fn create_user(&self, job: Json<Job>) -> CreateJobResponse {
        let mut job = job.0;
        job.id = Uuid::new_v4().to_string();
        job.status = String::from("RUNNING");
        let res = redis_add_task(&job, "todo-list").await;

        match res {
            Ok(_) => CreateJobResponse::Ok(Json(job)),
            Err(_) => CreateJobResponse::InternalError,
        }
    }

    #[oai(path = "/jobs/:job_id", method = "get")]
    async fn index(
        &self,
        #[oai(name = "job_id", in = "path")] job_id: String, // in="query" means this parameter is parsed from Url
    ) -> FindJobResponse {
        // PlainText is the response type, which means that the response type of the API is a string, and the Content-Type is `text/plain`
        // let jobs = self.jobs.lock().await;
        // match jobs.get(job_id as usize) {
        //     Some(job) => {
        //         FindJobResponse::Ok(Json(job.clone()))
        //     },
        //     None => FindJobResponse::NotFound,
        // }
        match redis_hset_query(&job_id).await {
            Ok(job) => FindJobResponse::Ok(Json(job.clone())),
            Err(_) => FindJobResponse::NotFound,
        }
    }

    /// Delete job by id
    #[oai(path = "/jobs/:job_id", method = "delete")]
    async fn delete_user(
        &self,
        #[oai(name = "job_id", in = "path")] job_id: String,
    ) -> DeleteJobResponse {
        // let mut jobs = self.jobs.lock().await;
        // let job_id = job_id as usize;
        // if jobs.contains(job_id) {
        //     jobs.remove(job_id);

        match self.index(job_id.clone()).await {
            FindJobResponse::Ok(_) => {
                let _ = redis_delete(job_id, "running-list").await;
                DeleteJobResponse::Ok
            },
            FindJobResponse::NotFound => DeleteJobResponse::NotFound,
        }
        // let _ = redis_delete(job_id, "running-list").await;
        // DeleteJobResponse::Ok
        // } else {
        //     DeleteJobResponse::NotFound
        // }
    }

    /// Update job by id
    #[oai(path = "/jobs/:job_id", method = "put")]
    async fn put_user(
        &self,
        #[oai(name = "job_id", in = "path")] job_id: String,
        update: Json<UpdateJob>,
    ) -> UpdateJobResponse {
        // let mut jobs = self.jobs.lock().await;
        // match jobs.get_mut(job_id as usize) {
        //     Some(job) => {
        //         if let Some(content) = update.0.content {
        //             job.content = content;
        //         }
        //         if let Some(schedule_type) = update.0.schedule_type {
        //             job.schedule_type = schedule_type;
        //         }
        //         if let Some(duration) = update.0.duration {
        //             job.duration = duration;
        //         }
        match self.index(job_id).await {
            FindJobResponse::Ok(job) => {
                let mut job = job.0;
                if let Some(content) = update.0.content {
                    job.content = content;
                }
                if let Some(schedule_type) = update.0.schedule_type {
                    job.schedule_type = schedule_type;
                }
                if let Some(duration) = update.0.duration {
                    job.duration = duration;
                }
                let _ = redis_update(&job, "running-list").await;
                UpdateJobResponse::Ok
            },
            FindJobResponse::NotFound => UpdateJobResponse::NotFound,
        }

            // }
            // None => 
    }
}

async fn redis_hset_query(job_id: &str) -> redis::RedisResult<Job> {
    let client = redis::Client::open("redis://127.0.0.1/").unwrap();
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
    Ok(job)
}

async fn redis_add_task(job: &Job, list: &str) -> redis::RedisResult<()> {
    let client = redis::Client::open("redis://127.0.0.1/").unwrap();
    let mut con = client.get_async_connection().await?;

    let new_job = format!(
        "{}::{}::{}::{}",
        job.id, job.content, job.schedule_type, job.duration
    );
    let _ = redis::cmd("RPUSH")
        .arg(&[list, &new_job])
        .query_async(&mut con)
        .await?;

    Ok(())
}

async fn redis_update(job: &Job, list: &str) -> redis::RedisResult<()> {
    let client = redis::Client::open("redis://127.0.0.1/").unwrap();
    let mut con = client.get_async_connection().await?;

    let new_job = format!(
        "{}::{}::{}::{}",
        job.id, job.content, job.schedule_type, job.duration
    );
    let update_job = format!("{}|update|{}", job.id, new_job);
    let _ = redis::cmd("RPUSH")
        .arg(&[list, &update_job])
        .query_async(&mut con)
        .await?;

    Ok(())
}

async fn redis_delete(id: String, list: &str) -> redis::RedisResult<()> {
    let client = redis::Client::open("redis://127.0.0.1/").unwrap();
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