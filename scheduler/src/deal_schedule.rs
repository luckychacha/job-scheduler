use chrono::Local;
use mobc_redis::redis::AsyncCommands;
use std::io::Error;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time;

use mobc_redis::mobc;
use mobc_redis::redis;
use mobc_redis::RedisConnectionManager;

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

// 每 10 秒扫描一次
// Todo-List，如果有新任务，通过 add_tasks 方法，将数据写入 HSET 中。
// 如果是循环任务，就根据 duration 设定一个 interval，并且进行循环，按时间间隔进行打印。
// 如果是仅一次的任务，就根据 duration 设定一个 sleep，延时打印。
// 由于时间关系，此处有一个可优化的点，就是每次运行时，可以记录当前的时间【 last tick 】以及执行的总次数【 ran times 】，
// 用他们可以发现任务未按时执行的情况，实现监控。
// Running-List 用于处理修改和删除请求。
// 将任务的 status 改为 STOPPED，保证老任务不会输出。
// 如果是更新任务，就会把新的任务重新写入 Todo-List，让任务按照新的规则跑起来。
// 如果是删除任务，就会停止循环。
pub async fn schedule_start() -> JoinHandle<Result<(), Error>> {
    let redis_pool = RedisPool::new().unwrap();

    tokio::spawn(async move {
        let mut get_task_interval = time::interval(Duration::from_secs(10));
        loop {
            get_task_interval.tick().await;
            async {
                println!("Start scan tasks in todo-list! now is {}", now());

                if let Ok(tasks) = get_todo_list_from_redis(redis_pool.clone()).await {
                    let _ = add_tasks(tasks, redis_pool.clone()).await;
                }

                println!("Start scan tasks in running-list! now is {}", now());

                if let Ok(tasks) = get_running_list_from_redis(redis_pool.clone()).await {
                    let _ = update_tasks(tasks, redis_pool.clone()).await;
                }
            }
            .await;
        }
    })
}

async fn get_todo_list_from_redis(redis_pool: RedisPool) -> redis::RedisResult<Vec<String>> {
    let mut con = redis_pool.clone().get_connection().await.unwrap();
    let result = con.lrange("todo-list", 0, -1).await.unwrap();

    //RPUSH -> LPOP remove tasks from todo list[todo-list]
    for task in &result {
        println!("task {} is removed from todo list", task);
        let mut con = redis_pool.clone().get_connection().await.unwrap();

        let _ = con.lpop("todo-list").await?;
    }
    Ok(result)
}

async fn add_tasks(tasks: Vec<String>, redis_pool: RedisPool) -> redis::RedisResult<()> {
    for task in tasks {
        let redis_pool = redis_pool.clone();
        let mut con = redis_pool.clone().get_connection().await.unwrap();

        let (id, content, schedule_type, duration, slab_idx) = parser_task(task);

        let params = &[
            ("id", &id),
            ("content", &content),
            ("schedule_type", &schedule_type),
            ("duration", &duration.to_string()),
            ("status", &String::from("RUNNING")),
            ("slab_idx", &slab_idx.to_string()),
        ];
        let _ = con.hset_multiple(id.clone(), params).await?;
        let _ = tokio::spawn(async move {
            if "OneShot" == schedule_type {
                time::sleep(Duration::from_secs(duration)).await;
                if !check_is_need_to_stop(&id, redis_pool.clone()).await {
                    println!(
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
                    // if check_is_need_to_stop_a(&id, redis_pool.clone()).await {
                    if check_is_need_to_stop(&id, redis_pool.clone()).await {
                        break;
                    }
                    async {
                        println!(
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
    }
    Ok(())
}

async fn check_is_need_to_stop(id: &str, redis_pool: RedisPool) -> bool {
    let job_status = get_job_status(id, redis_pool.clone()).await;

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

// async fn check_is_need_to_stop(id: &str) -> bool {
//     let job_status = get_job_status(id).await;

//     match job_status {
//         Ok(job_status) => {
//             if "RUNNING" == &job_status {
//                 return false;
//             }
//             true
//         }
//         Err(_) => true,
//     }
// }

async fn get_job_status(id: &str, redis_pool: RedisPool) -> redis::RedisResult<String> {
    let mut con = redis_pool.clone().get_connection().await.unwrap();

    let result: String = con.hget(id, "status").await?;
    Ok(result)
}

async fn update_tasks(tasks: Vec<String>, redis_pool: RedisPool) -> redis::RedisResult<()> {
    for task in tasks {
        let redis_pool = redis_pool.clone();
        let mut con = redis_pool.get_connection().await.unwrap();
        let (update_id, schedule_type, content) = parse_running_task(task);

        // 无论删除还是修改，都需要把任务停止
        let _ = con.hset(&update_id, "status", "STOPPED").await?;

        if "update" == &schedule_type {
            let (_, content, schedule_type, sec, slab_idx) = parser_task(content);

            let new_job = format!(
                "{}::{}::{}::{}::{}",
                update_id, content, schedule_type, sec, slab_idx
            );

            let _ = con.rpush("todo-list", &new_job).await?;
        }
    }
    Ok(())
}

// async fn stop_task(id: &str, redis_pool: RedisPool) -> redis::RedisResult<()> {
//     let client = redis::Client::open("redis://127.0.0.1").unwrap();
//     let mut con = client.get_async_connection().await?;

//     let _ = redis::cmd("HSET")
//         .arg(id)
//         .arg("status")
//         .arg("STOPPED")
//         .query_async(&mut con)
//         .await?;

//     Ok(())
// }

async fn get_running_list_from_redis(redis_pool: RedisPool) -> redis::RedisResult<Vec<String>> {
    let mut con = redis_pool.clone().get_connection().await.unwrap();
    let result: Vec<String> = con.lrange("running-list", 0, -1).await?;

    //RPUSH -> LPOP remove tasks from running list[running-list]
    for task in &result {
        println!("task {} is added into running list", task);

        let mut con = redis_pool.clone().get_connection().await.unwrap();
        let _ = con.lpop("running-list").await?;
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
    // ""
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