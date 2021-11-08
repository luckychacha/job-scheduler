use std::time::Duration;

use chrono::Local;
use tokio::time;

async fn async_main() {
    // 创建一个永远不会结束的任务，保证 Scheduler 会一直运行。
    let start = tokio::spawn(async move {
        println!("Start");
        let mut init_interval = time::interval(Duration::from_secs(100));

        loop {
            init_interval.tick().await;
        }
    });

    // 每 10 秒扫描一次 Todo-List，如果有新任务，通过 add_tasks 方法，将数据写入 HSET 中。
    // 如果是循环任务，就根据 duration 设定一个 interval，并且进行循环，按时间间隔进行打印。
    // 如果是仅一次的任务，就根据 duration 设定一个 sleep，延时打印。
    // 由于时间关系，此处有一个可优化的点，就是每次运行时，可以记录当前的时间【 last tick 】以及执行的总次数【 ran times 】，
    // 用他们可以发现任务未按时执行的情况，实现监控。
    tokio::spawn(async move {
        let mut get_task_interval = time::interval(Duration::from_secs(10));
        loop {
            println!("Start scan tasks in todo-list! now is {}", now());

            get_task_interval.tick().await;
            async {
                if let Ok(tasks) = get_todo_list_from_redis().await {
                    let _ = add_tasks(tasks).await;
                }
            }
            .await;
        }
    });

    // Running-List 用于处理修改和删除请求。
    // 每 10 秒扫描一次 Running-List，如果有任务，通过 update_tasks 方法，
    // 将任务的 status 改为 STOPPED，保证老任务不会输出。
    // 如果是更新任务，就会把新的任务重新写入 Todo-List，让任务按照新的规则跑起来。
    // 如果是删除任务，就会停止循环。
    tokio::spawn(async move {
        let mut get_update_task_interval = time::interval(Duration::from_secs(10));
        loop {
            println!("Start scan tasks in running-list! now is {}", now());

            get_update_task_interval.tick().await;
            async {
                if let Ok(tasks) = get_running_list_from_redis().await {
                    let _ = update_tasks(tasks).await;
                }
            }
            .await;
        }
    });
    let _ = start.await;
}

async fn get_todo_list_from_redis() -> redis::RedisResult<Vec<String>> {
    let client = redis::Client::open("redis://redis").unwrap();
    let mut con = client.get_async_connection().await?;

    let result: Vec<String> = redis::cmd("LRANGE")
        .arg(&["todo-list", "0", "-1"])
        .query_async(&mut con)
        .await?;

    //RPUSH -> LPOP remove tasks from todo list[todo-list]
    for task in &result {
        println!("task {} is removed from todo list", task);

        let _ = redis::cmd("LPOP")
            .arg(&["todo-list"])
            .query_async(&mut con)
            .await?;
    }
    Ok(result)
}

async fn add_tasks(tasks: Vec<String>) -> redis::RedisResult<()> {
    let client = redis::Client::open("redis://redis").unwrap();
    let mut con = client.get_async_connection().await?;

    for task in tasks {
        let (id, content, schedule_type, duration, slab_idx) = parser_task(task);

        let _ = redis::cmd("HMSET")
            .arg(&(id))
            .arg(&["id", &id])
            .arg(&["content", &content])
            .arg(&["schedule_type", &schedule_type])
            .arg(&["duration", &duration.to_string()])
            .arg(&["status", "RUNNING"])
            .arg(&["slab_idx", &slab_idx.to_string()])
            .query_async(&mut con)
            .await?;

        let _ = tokio::spawn(async move {
            if "OneShot" == schedule_type {
                time::sleep(Duration::from_secs(duration)).await;
                if !check_is_need_to_stop(&id).await {
                    println!(
                        "  ****  id: {}, content: {}, type: {} now is {}  ****  ",
                        id,
                        content,
                        schedule_type,
                        now()
                    );
                }
                // stop_task(&id);
            } else if "Repeated" == schedule_type {
                let mut interval = time::interval(Duration::from_secs(duration));
                loop {
                    interval.tick().await;
                    if check_is_need_to_stop(&id).await {
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
    let client = redis::Client::open("redis://redis").unwrap();
    let mut con = client.get_async_connection().await?;
    let result: String = redis::cmd("HGET")
        .arg(&[id, "status"])
        .query_async(&mut con)
        .await?;
    Ok(result)
}

async fn update_tasks(tasks: Vec<String>) -> redis::RedisResult<()> {
    let client = redis::Client::open("redis://redis").unwrap();
    let mut con = client.get_async_connection().await?;
    for task in tasks {
        let (update_id, schedule_type, content) = parse_running_task(task);

        // 无论删除还是修改，都需要把任务停止
        stop_task(&update_id).await?;

        if "update" == &schedule_type {
            let (_, content, schedule_type, sec, slab_idx) = parser_task(content);

            let new_job = format!(
                "{}::{}::{}::{}::{}",
                update_id, content, schedule_type, sec, slab_idx
            );

            let _ = redis::cmd("RPUSH")
                .arg(&["todo-list", &new_job])
                .query_async(&mut con)
                .await?;
        }
    }
    Ok(())
}

async fn stop_task(id: &str) -> redis::RedisResult<()> {
    let client = redis::Client::open("redis://redis").unwrap();
    let mut con = client.get_async_connection().await?;

    let _ = redis::cmd("HSET")
        .arg(&id)
        .arg("status")
        .arg("STOPPED")
        .query_async(&mut con)
        .await?;

    Ok(())
}

async fn get_running_list_from_redis() -> redis::RedisResult<Vec<String>> {
    let client = redis::Client::open("redis://redis").unwrap();
    let mut con = client.get_async_connection().await?;

    let result: Vec<String> = redis::cmd("LRANGE")
        .arg(&["running-list", "0", "-1"])
        .query_async(&mut con)
        .await?;

    //RPUSH -> LPOP remove tasks from running list[running-list]
    for task in &result {
        println!("task {} is added into running list", task);

        let _ = redis::cmd("LPOP")
            .arg(&["running-list"])
            .query_async(&mut con)
            .await?;
    }
    Ok(result)
}

#[tokio::main]
async fn main() {
    async_main().await;
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
