use tokio::net::{TcpListener, TcpStream};

use futures::channel::mpsc::UnboundedReceiver;
use futures::sink::SinkExt;
use futures::stream::StreamExt;

use tokio::sync::mpsc;
use tokio_tungstenite::{tungstenite, WebSocketStream};
use tungstenite::Message as WsMessage;

use anyhow::{anyhow, Result};

use server::Server;

type Socket = WebSocketStream<TcpStream>;

async fn message_loop(
    server: &Server,
    socket: Socket,
    peer_id: u32,
    ws_rx: UnboundedReceiver<WsMessage>,
) -> Result<()> {
    let (mut ws_sink, ws_stream) = socket.split();

    let mut ws_rx = ws_rx.fuse();

    let mut ws_stream = ws_stream.fuse();
    loop {
        let ws_msg: Option<WsMessage> = futures::select! {
            ws_msg = ws_stream.select_next_some() => {
                match ws_msg? {
                    WsMessage::Text(text) => {
                        // println!("\nMessage Received: {}\n", &text);
                        server.handle_message(&text, peer_id)?;
                        None
                    },
                    _ => None
                }
            },
            ws_msg = ws_rx.select_next_some() => {
                Some(ws_msg)
            },
            complete => break
        };
        if let Some(ws_msg) = ws_msg {
            ws_sink.send(ws_msg).await?;
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    let addr = "127.0.0.1:8765".to_string();

    let listener = TcpListener::bind(&addr).await.unwrap();
    println!("\nListening on: {}", addr);

    let server = Server::new().unwrap();

    while let Ok((stream, addr)) = listener.accept().await {
        let server = server.clone();

        tokio::spawn(async move {
            // Register the incoming connection with a Peer_ID
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            println!("Websocket connection established: {}\n", &addr);

            let msg = socket
                .next()
                .await
                .ok_or_else(|| anyhow!("Did not receive Hello"))
                .unwrap()
                .unwrap();

            // let (tx, rx) = mpsc::channel::<WsMessage>(10);
            let (peer_id, ws_rx) = server.register_peer(msg, addr).unwrap();

            // Let the peer know they're registered
            socket
                .send(WsMessage::Text("Hello".to_string()))
                .await
                .unwrap();

            println!("Peer {} registered\n", &peer_id);

            if let Err(_) = message_loop(&server, socket, peer_id, ws_rx).await {
                server.remove_peer(peer_id).await.unwrap();
            }

            // If we are reaching this point - the peer has left
            // {
            //     let mut peers = server.peers.lock().unwrap();

            //     let peer = match peers.get(&peer_id).map(|peer| peer.to_owned()) {
            //         Some(room) => Some(room),
            //         _ => None,
            //     }
            //     .unwrap();

            //     match peer.status.lock().map(|id| *id).unwrap() {
            //         Some(room_id) => {
            //             let mut rooms = server.rooms.lock().unwrap();
            //             let room = rooms.get_mut(&room_id).unwrap();
            //             if room.len() == 1 {
            //                 println!("Last peer in the room {} left, destroying room...", room_id);

            //                 room.remove(0);
            //                 rooms.remove(&room_id);
            //             } else {
            //                 println!("Room {} cleaned for peer {}", room_id, peer_id);

            //                 room.retain(|val| val != &peer_id);

            //                 // FIX: We only need to inform the server present in the ROOM
            //                 room.iter()
            //                     .map(|other| peers.get(&other).unwrap().to_owned())
            //                     .for_each(move |peer| {
            //                         let tx = &peer.tx.lock().unwrap();
            //                         tx.unbounded_send(WsMessage::Text(format!(
            //                             "ROOM_PEER_LEFT {}",
            //                             peer_id
            //                         )))
            //                         .with_context(|| format!("Failed to send message on channel"))
            //                         .unwrap();
            //                     });
            //             }
            //         }
            //         None => (),
            //     };

            //     println!("Peer {} left. Removing..", &peer_id);
            //     peers.remove(&peer_id);
            //     // println!("PEERS: {:?}", peers);

            //     // TODO - DONE
            //     // 1) Broadcast message to the MCU that the peer has left
            //     // 2) Cleanup state - ROOM maintenance
            // }
        });
    }
}
