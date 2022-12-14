//! websocket client program
//!
//! connect massive web socket client and broadcast message for benchmarking
//! `RUST_LOG=info cargo run --example ws_client`

use chrono::Utc;
use futures::{SinkExt, StreamExt};
use log::{debug, info};
use rand::{distributions::Uniform, prelude::Distribution};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::{self, Instant};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const WS_ENDPOINT: &str = "ws://localhost:3000/websocket";

#[derive(Debug)]
pub struct Status(pub i64, pub i64, pub u32);

const CLIENT_COUNT: u64 = 10000;
const CLIENT_CREATING_PERIOD: u64 = 100;
const CHAT_SENDING_PERIOD: u64 = 20;

#[tokio::main]
async fn main() {
    // RUST_LOG 가 없으면 기본 레벨 지정
    env_logger::builder()
        .parse_filters(&std::env::var("RUST_LOG").unwrap_or_else(|_| "debug".into()))
        .format_timestamp_millis()
        .init();

    // 스레드간 공유되는 상태로 수신 메시지 갯수 송신 메시지 갯수
    let status: Arc<Mutex<Status>> = Arc::new(Mutex::new(Status(0, 0, 0)));
    let mut handles = vec![];
    let cloned_status = status.clone();

    let show_status = tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            info!("status: {:?}", cloned_status.lock().unwrap(),);
        }
    });
    handles.push(show_status);

    let mut rng = rand::thread_rng();
    let random_number: Uniform<i32> = Uniform::from(1..1000);

    for c in 0..CLIENT_COUNT {
        let enable_send = if c < 100 && random_number.sample(&mut rng) <= 100 {
            // 클라 100개 미만일시 10프로 확률로 전송 기능 활성화
            true
        } else {
            // 100개 이상이면 1프로로 확률 변경
            c >= 100 && random_number.sample(&mut rng) <= 10
        };
        // 0.1 초씩 클라이언트 증가
        time::sleep(Duration::from_millis(CLIENT_CREATING_PERIOD)).await;
        let cloned_status = status.clone();
        let h = tokio::spawn(async move {
            run_test(cloned_status.clone(), enable_send).await;
        });
        handles.push(h);
    }

    for h in handles {
        h.await.unwrap();
    }
    // let a = futures::future::join_all(handles);
}

pub async fn run_test(status: Arc<Mutex<Status>>, enable_send: bool) {
    // tasks 수 더하기
    let cloned_status = status.clone();
    cloned_status.lock().unwrap().2 += 1;
    drop(cloned_status);

    // 웹소켓 클라 만들어서 인증 하고 채팅 전송 그리고 수신시 수신 메시지 갯수 더하기
    let (ws_stream, _) = connect_async(WS_ENDPOINT).await.expect("Failed to connect");

    let (mut write, mut read) = ws_stream.split();

    debug!("수신 시작");
    let cloned_status = status.clone();
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(_msg)) = read.next().await {
            // info!("수신 {:?}", msg);
            cloned_status.lock().unwrap().0 += 1;
        }
    });

    // 인증 유저 아이디 랜덤으로 다르게 지정
    let msg = format!(
        r#"{{"o":10,"n":"user_name","i":"s{}"}}"#,
        rand::random::<i64>()
    );
    write.send(Message::from(msg)).await.unwrap();
    // 채팅방 조인
    write
        .send(Message::from(r#"{"o":181,"r":"room_id"}"#))
        .await
        .unwrap();

    // 0.3초 마다 랜덤 숫자 넣은 채팅 전송
    let cloned_status = status.clone();
    let stop_watch = Instant::now();
    let send_task = tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_millis(CHAT_SENDING_PERIOD));
        if !enable_send {
            // 송신 하는애 아니면 그냥 리턴
            return;
        }
        loop {
            interval.tick().await;
            let now = Utc::now().naive_utc().timestamp_millis();
            let millisecond_part = now % 1000;
            // 현재 밀리세컨드가 300초 이하면 송신 한다 송실한 클라도 랜덤이지만 매 시간마다
            // 송신 하는 행위도 랜덤으로 하도록 지정
            if millisecond_part < 333 && stop_watch.elapsed().as_millis() > 2000 {
                // 2초 이내면 실행 안함 최초 실행을 막고 지정된 interval 대로 메시지 송신 하기 위함
                let msg = format!(
                    r#"{{"o":101, "r":"room_id", "c":"{} 안녕 나는 {} 이야"}}"#,
                    rand::random::<i64>(),
                    rand::random::<i64>()
                );
                write.send(Message::from(msg)).await.unwrap();
                cloned_status.lock().unwrap().1 += 1;
            }
        }
    });
    recv_task.await.unwrap();
    send_task.await.unwrap();
}
