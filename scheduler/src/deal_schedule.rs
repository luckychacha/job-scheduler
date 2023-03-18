use chrono::Local;
use std::time::Duration;
use std::{collections::HashMap, io::Error};
use tokio::task::JoinHandle;
use tokio::time;

use mobc_redis::redis;
use mobc_redis::redis::AsyncCommands;

use tracing::{debug, info, instrument};

use crate::connection;

struct Task {
    id: String,
    handle: JoinHandle<()>,
}

impl Task {
    fn new(id: String, handle: JoinHandle<()>) -> Self {
        Task { id, handle }
    }
}

#[instrument(level = "debug")]
pub async fn schedule_start() -> JoinHandle<Result<(), Error>> {
    let mut all_tasks: HashMap<String, Task> = HashMap::new();
    tokio::spawn(async move {
        let mut get_task_interval = time::interval(Duration::from_secs(10));
        loop {
            get_task_interval.tick().await;
            async {
                debug!("Start scan tasks in todo-list! now is {}", now());

                if let Ok(tasks) = get_todo_list_from_redis().await {
                    let batch_new_tasks = add_tasks(tasks).await;
                    if let Ok(new_tasks) = batch_new_tasks {
                        all_tasks.extend(new_tasks);
                    }
                }
                debug!("Start scan tasks in running-list! now is {}", now());

                if let Ok(tasks) = get_running_list_from_redis().await {
                    let _ = update_tasks(tasks).await;
                }
            }
            .await;
        }
    })
}

#[instrument(level = "debug")]
async fn get_todo_list_from_redis() -> redis::RedisResult<Vec<String>> {
    let result = connection().await.lrange("todo-list", 0, -1).await.unwrap();

    //RPUSH -> LPOP remove tasks from todo list[todo-list]
    for task in &result {
        debug!("task {} is removed from todo list", task);
        let _ = connection().await.lpop("todo-list").await?;
    }
    Ok(result)
}

#[instrument(level = "info")]
async fn add_tasks(tasks: Vec<String>) -> redis::RedisResult<HashMap<String, Task>> {
    let mut tmp: HashMap<String, Task> = HashMap::new();
    for task in tasks {
        let (id, content, schedule_type, duration, slab_idx) = parser_task(task);

        let params = &[
            ("id", &id),
            ("content", &content),
            ("schedule_type", &schedule_type),
            ("duration", &duration.to_string()),
            ("status", &String::from("RUNNING")),
            ("slab_idx", &slab_idx.to_string()),
        ];
        let _ = connection().await.hset_multiple(id.clone(), params).await?;
        let task_id = id.clone();

        let handle = tokio::spawn(async move {
            if "OneShot" == schedule_type {
                time::sleep(Duration::from_secs(duration)).await;
                if !check_is_need_to_stop(&id).await {
                    info!(
                        "  ****  id: {}, content: {}, type: {} now is {}  ****  ",
                        id,
                        content,
                        schedule_type,
                        now()
                    );
                }
            } else if "Repeated" == schedule_type {
                let mut interval = time::interval(Duration::from_secs(duration));
                loop {
                    interval.tick().await;
                    if check_is_need_to_stop(&id).await {
                        break;
                    }
                    async {
                        info!(
                            "  ****  id: {}, content: {}, type: {}, now is {}  ****  ",
                            id,
                            content,
                            schedule_type,
                            now()
                        );
                    }
                    .await;
                }
            }
        });
        tmp.insert(task_id.clone(), Task::new(task_id.clone(), handle));
    }
    Ok(tmp)
}

async fn check_is_need_to_stop(id: &str) -> bool {
    let job_status = get_job_status(id).await;

    match job_status {
        Ok(job_status) => {
            if "RUNNING" == &job_status {
                return false;
            }
            true
        }
        Err(_) => true,
    }
}

async fn get_job_status(id: &str) -> redis::RedisResult<String> {
    let result: String = connection().await.hget(id, "status").await?;
    Ok(result)
}

async fn update_tasks(tasks: Vec<String>) -> redis::RedisResult<()> {
    for task in tasks {
        let (update_id, schedule_type, content) = parse_running_task(task);

        let _ = connection()
            .await
            .hset(&update_id, "status", "STOPPED")
            .await?;

        if "update" == &schedule_type {
            let (_, content, schedule_type, sec, slab_idx) = parser_task(content);

            let new_job = format!(
                "{}::{}::{}::{}::{}",
                update_id, content, schedule_type, sec, slab_idx
            );

            let _ = connection().await.rpush("todo-list", &new_job).await?;
        }
    }
    Ok(())
}

#[instrument(level = "debug")]
async fn get_running_list_from_redis() -> redis::RedisResult<Vec<String>> {
    let result: Vec<String> = connection().await.lrange("running-list", 0, -1).await?;

    //RPUSH -> LPOP remove tasks from running list[running-list]
    for task in &result {
        debug!("task {} is added into running list", task);

        let _ = connection().await.lpop("running-list").await?;
    }
    Ok(result)
}

fn now() -> String {
    let fmt = "%Y-%m-%d %H:%M:%S";
    Local::now().format(fmt).to_string()
}

fn parser_task(task: String) -> (String, String, String, u64, i64) {
    let mut id = String::from("");
    let mut content = String::from("");
    let mut schedule_type = String::from("");
    let mut duration = 0;
    let mut slab_idx = 0;
    let _: Vec<&str> = task
        .split("::")
        .enumerate()
        .map(|(idx, item)| {
            match idx {
                0 => id = String::from(item),
                1 => content = String::from(item),
                2 => schedule_type = String::from(item),
                3 => duration = item.parse::<u64>().unwrap(),
                4 => slab_idx = item.parse::<i64>().unwrap(),
                _ => {}
            }
            ""
        })
        .collect();
    (id, content, schedule_type, duration, slab_idx)
}

fn parse_running_task(task: String) -> (String, String, String) {
    let mut update_id = String::from("");
    let mut content = String::from("");
    // update / delete
    let mut schedule_type = String::from("");

    let _: Vec<&str> = task
        .split("|")
        .enumerate()
        .map(|(idx, item)| {
            match idx {
                0 => update_id = String::from(item),
                1 => schedule_type = String::from(item),
                2 => content = String::from(item),
                _ => {}
            }
            ""
        })
        .collect();
    (update_id, schedule_type, content)
}
