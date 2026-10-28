//! A tiny CONNECT proxy for the proxy tests: it tunnels to a fixed upstream and records every
//! CONNECT (target and raw head).
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

pub(crate) struct FakeProxy {
    pub addr: SocketAddr,
    /// (target `host:port`, raw request head) of each CONNECT.
    pub connects: Arc<Mutex<Vec<(String, String)>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeProxy {
    pub(crate) async fn start(upstream: SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let connects = Arc::new(Mutex::new(Vec::new()));
        let task = {
            let connects = Arc::clone(&connects);
            tokio::spawn(async move {
                loop {
                    let Ok((mut client, _)) = listener.accept().await else {
                        return;
                    };
                    let connects = Arc::clone(&connects);
                    tokio::spawn(async move {
                        let mut head = Vec::new();
                        let mut byte = [0_u8; 1];
                        while !head.ends_with(b"\r\n\r\n") && head.len() < 8192 {
                            if client.read_exact(&mut byte).await.is_err() {
                                return;
                            }
                            head.push(byte[0]);
                        }
                        let head = String::from_utf8_lossy(&head).into_owned();
                        let target = head
                            .lines()
                            .next()
                            .and_then(|line| line.strip_prefix("CONNECT "))
                            .and_then(|rest| rest.split(' ').next())
                            .unwrap_or_default()
                            .to_owned();
                        connects.lock().unwrap().push((target, head));
                        let Ok(mut upstream) = tokio::net::TcpStream::connect(upstream).await
                        else {
                            return;
                        };
                        if client
                            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                            .await
                            .is_err()
                        {
                            return;
                        }
                        let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
                    });
                }
            })
        };
        Self {
            addr,
            connects,
            task,
        }
    }
}
