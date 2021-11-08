# Job Schedule

## 项目介绍

### 目录结构

```shell

├─1.http_server
│  │
│  ├─main.rs
│  └─── 此模块代码主要实现 server 端的各类业务操作：添加任务、修改任务、查询任务、删除任务      
│        
├─2.scheduler
│  │  
│  ├─main.rs
│  └─── 此模块代码主要实现任务调度。 
│
├─3.rust
│  │  
│  ├─Dockerfile
│  └─── 此目录存放的是 Rust 的容器的构建步骤，docker-compose 的一部分。        
│         
└─4.docker-compose.yml
   │     
   └─── 此文件是 docker-compose 的配置文件，主要包含 Redis 容器和 Rust 容器。
```

### 部署方式



### 接口调用

```shell

# 1.添加任务，接口调用方式如下：
curl --location --request POST 'http://localhost:3000/api/jobs' \
--header 'Content-Type: application/json' \
--data-raw '{
  "content": "添加任务",
  "scheduleType": "Repeated",
  "duration": 5,
  "status": "RUNNING"
}'

# 2.添加接口返回内容【 json 格式】
{"content":"添加任务","duration":5,"id":"fc017bbb-465c-4cd0-95de-a63216e4fd85","scheduleType":"Repeated","status":"RUNNING"}


# 3.使用返回的 id 作为查询条件，调用查询接口。
curl --location --request GET 'http://localhost:3000/api/jobs/fc017bbb-465c-4cd0-95de-a63216e4fd85' \
--header 'Content-Type: application/json'

# 4.查询接口返回任务详情【 json 格式】
{"content":"添加任务","duration":5,"id":"fc017bbb-465c-4cd0-95de-a63216e4fd85","scheduleType":"Repeated","status":"RUNNING"}

# 5.修改任务，接口调用方式如下：
curl --location --request PUT 'http://localhost:3000/api/jobs/fc017bbb-465c-4cd0-95de-a63216e4fd85' \
--header 'Content-Type: application/json' \
--data-raw '{
  "content": "修改任务",
  "scheduleType": "Repeated",
  "duration": 10
}'

# 6.删除任务，接口调用方式如下：
curl --location --request DELETE 'http://localhost:3000/api/jobs/fc017bbb-465c-4cd0-95de-a63216e4fd85' \
--data-raw ''

```