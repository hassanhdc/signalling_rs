use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex, Weak},
};

use futures_core::{FusedStream, Stream};
use serde_derive::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};

use futures::channel::mpsc::{self, UnboundedReceiver};
use futures::{channel::mpsc::UnboundedSender, stream::StreamExt};
use futures::{
    sink::{Sink, SinkExt},
    stream::SplitStream,
};

use tokio_tungstenite::{tungstenite, WebSocketStream};
use tungstenite::Error as WsError;
use tungstenite::Message as WsMessage;

use anyhow::{anyhow, bail, Context, Result};

type Socket = WebSocketStream<TcpStream>;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SocketMsg {
    RoomPeer {
        // Messages starting wtih ROOM_PEER. Do not warrant the any action
        // from the Socket Server other than routing the message itself
        //
        // TYPES:
        // 1) ROOM_PEER_MSG - is either an SDP or ICE
        // 2) ROOM_PEER_JOINED - notification that a peer has joined the ROOM
        // 3) ROOM_PEER_LEFT - notification that a peer has left the ROOM
        #[serde(rename = "type")]
        type_: String,
        to: u32,
        from: u32,
    },
    Server {
        // Messages meant specifically for/from the "MCU". Typically conveying
        // some communication prompting the "MCU" to take some action/measure
        //
        // TYPES:
        // 1) SERVER_RESTART - indicating that all peers from a particular ROOM have left
        // 2) SERVER_CREATE - notify MCU to ready a new socket to serve a new ROOM session
        // 3) SERVER_INVITE - notify the MCU to join a particular ROOM session
        // ... Can be added to
        #[serde(rename = "type")]
        type_: String,
        to: u32,
        from: u32,
    },
    RoomCommand {
        // Messages meant for the "Socket Server" for ROOM creation
        room_id: u32,
        to: u32,
        from: u32,
    },
}

#[derive(Debug, Clone)]
struct Peer(Arc<PeerInner>);

#[derive(Debug)]
struct PeerInner {
    id: u32,
    addr: SocketAddr,
    status: Mutex<Option<u32>>,
    tx: Arc<Mutex<mpsc::UnboundedSender<WsMessage>>>,
}

#[derive(Debug, Clone)]
struct Server(Arc<ServerInner>);

#[derive(Debug)]
struct ServerInner {
    //  peers: {uid: [Peer tx,
    //                remote_address,
    //                <"room_id"|None>]}
    peers: Mutex<HashMap<u32, Peer>>,
    //
    //
    // rooms: {room_id: [peer1_id, peer2_id, peer3_id, ...]}
    rooms: Mutex<HashMap<u32, Vec<u32>>>,
    //
    //
    // room_servers: {server_id: [room_id1, room_id2 ...]}
    room_servers: Mutex<HashMap<u32, Vec<u32>>>,
    // server: TcpListener,
    // ws_msg_tx: Arc<Mutex<mpsc::UnboundedSender<WsMessage>>>,
}

impl std::ops::Deref for Peer {
    type Target = PeerInner;

    fn deref(&self) -> &PeerInner {
        &self.0
    }
}

impl std::ops::Deref for Server {
    type Target = ServerInner;

    fn deref(&self) -> &ServerInner {
        &self.0
    }
}

impl Peer {
    fn new(
        id: u32,
        addr: SocketAddr,
        status: Option<u32>,
    ) -> Result<(Self, UnboundedReceiver<WsMessage>)> {
        let (tx, rx) = mpsc::unbounded::<WsMessage>();

        let status = Mutex::new(status);

        let peer = Peer(Arc::new(PeerInner {
            id,
            addr,
            status,
            tx: Arc::new(Mutex::new(tx)),
        }));

        Ok((peer, rx))
    }
}

impl Server {
    fn new() -> Result<Self> {
        // let (ws_msg_tx, ws_msg_rx) = mpsc::unbounded::<WsMessage>();

        let server = Server(Arc::new(ServerInner {
            peers: Mutex::new(HashMap::new()),
            rooms: Mutex::new(HashMap::new()),
            room_servers: Mutex::new(HashMap::new()),
        }));

        Ok(server)
    }
    fn handle_message(&self, message: &str, from_id: u32) -> Result<()> {
        if message.starts_with("MSG_ROOM_PEER") {
            // This condition is met under the following assumption:
            // The peer sending the message is already in a ROOM - Assert the following
            // i.e. peer.status != None

            // Action:
            // Forward message addressed to the room peer - if not present, inform the sending peer

            let mut split = message["MSG_ROOM_PEER ".len()..].splitn(2, ' ');
            let to_id = split
                .next()
                .and_then(|s| str::parse::<u32>(s).ok())
                .ok_or_else(|| anyhow!("Cannot parse PEER ID from message"))
                .unwrap();

            let msg = split
                .next()
                .ok_or_else(|| anyhow!("Cannot parse peer message"))?;

            println!("{} -> {}: {}", from_id, to_id, msg);

            let peers = self.peers.lock().unwrap();

            let (peer, msg) = if let None = peers.get(&to_id) {
                // If peer with "to_id" is not connected, alter the message and return the the same peer
                (
                    peers
                        .get(&from_id)
                        .ok_or_else(|| anyhow!("Cannot find peer {}", from_id))?
                        .to_owned(),
                    format!("ERROR peer {} not found", to_id),
                )
            } else {
                // If peer with "to_id" is connected, message remains same and the return "to_id" peer
                (
                    peers
                        .get(&to_id)
                        .ok_or_else(|| anyhow!("Cannot find peer {}", to_id))?
                        .to_owned(),
                    msg.to_string(),
                )
            };
            drop(peers);

            // Access the channel for the returned peer to we can forward them the message
            let tx = &peer.tx.lock().unwrap();
            tx.unbounded_send(WsMessage::Text(msg.into()))
                .with_context(|| format!("Failed to message on channel"))?;

            // let rooms = self.rooms.lock().unwrap();
        } else if message.starts_with("CMD_ROOM") {
            // This condition is met under the following assumption:
            // The peer sending the message is not in a ROOM - Assert the following
            // i.e. peer.status == None

            // Action:
            // Create the ROOM by amending the server state, let the peer know the ROOM was created

            // TODO:
            // Implement MCU logic for ROOM creation i.e. MCU creates aliases for every room created
            // The alias acts as a peer where peer_id == room_id and is the facilitator of audio/video conference

            let mut msg = message["CMD_ROOM".len()..].split("_").skip(1);
            let mut split = msg
                .next()
                .ok_or_else(|| anyhow!("Cannot split command message"))
                .unwrap()
                .splitn(2, " ");

            let command = split
                .next()
                .ok_or_else(|| anyhow!("Cannot pase command message"))
                .unwrap();
            let room_id = split
                .next()
                .and_then(|s| str::parse::<u32>(s).ok())
                .ok_or_else(|| anyhow!("Cannot parse ROOM ID from message"))
                .unwrap();

            println!("\nGOT COMMAND: {}\nROOM: {}\n", command, room_id);

            // We are now handling the CMD_ROOM 'CREATE' and 'JOIN' cases within the same branch for brevity and conciseness
            // since we will be checking/modifying related state, why not group the code at one place

            // TODO: Conditionallly handle the cases for "ROOM CREATE" AND "ROOM JOIN"

            // let mut split = message["CMD_ROOM_CREATE ".len()..].splitn(2, ' ');
            // let room_id = split
            //     .next()
            //     .and_then(|s| str::parse::<u32>(s).ok())
            //     .ok_or_else(|| anyhow!("Cannot parse ROOM ID from message"))
            //     .unwrap();

            println!("{}: CREATE ROOM {}", from_id, room_id);

            let peers = self.peers.lock().unwrap();
            let mut rooms = self.rooms.lock().unwrap();

            let peer = peers.get(&from_id).unwrap().to_owned();

            // Ensure peers status is None
            if let Some(status) = *peer.status.lock().unwrap() {
                bail!("Peer {} is already in ROOM {}", from_id, status);
            }

            let msg = if let None = rooms.get(&room_id) {
                // If the room_id is unique, create the ROOM
                rooms.insert(room_id, vec![from_id]);
                // Enter the peer into the ROOM
                let peer = peers.get(&from_id).unwrap().to_owned();
                *peer.status.lock().unwrap() = Some(room_id);
                format!("ROOM_OK")
            } else {
                format!("ERROR ROOM {} already exists", room_id)
            };
            // Debug
            println!("\nPEERS: {:?}\nROOMS:{:?}\n", peers, rooms);
            drop(peers);
            drop(rooms);

            let tx = &peer.tx.lock().unwrap();
            tx.unbounded_send(WsMessage::Text(msg.into()))
                .with_context(|| format!("Failed to message on channel"))?;
        }
        // else if message.starts_with("CMD_ROOM_JOIN") {
        //     let mut split = message["CMD_ROOM_JOIN ".len()..].splitn(2, ' ');
        // }
        Ok(())
    }
}

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
                        println!("\nMessage Received: {}\n", &text);
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

// async fn ws_read(ws_stream: SplitStream<Socket>) -> Result<()> {
//     let mut ws_stream = ws_stream.fuse();
//     loop {
//         let ws_msg: Option<WsMessage> = futures::select! {
//             ws_msg = ws_stream.select_next_some() => {
//                 match ws_msg? {
//                 WsMessage::Text(text) => {
//                     println!("\nMessage Received: {}\n", text);
//                     None
//                 },
//                 _ => None
//             }
//             }
//             complete => break
//         };
//     }
//     Ok(())
// }

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

            let peer_id = msg
                .into_text()
                .map(|text| {
                    if text.starts_with("Hello") {
                        let mut split = text["Hello ".len()..].splitn(2, ' ');
                        let peer_id = split
                            .next()
                            .and_then(|s| str::parse::<u32>(s).ok())
                            .ok_or_else(|| anyhow!("Cannot parse peer id"))
                            .unwrap();
                        println!("Peer wants to register with ID: {}", &peer_id);
                        Some(peer_id)
                    } else {
                        None
                    }
                })
                .expect("Server did not say Hello");

            // Let the peer know they're registered
            socket
                .send(WsMessage::Text("Hello".to_string()))
                .await
                .unwrap();

            let ws_rx = if let Some(peer_id) = peer_id {
                let mut peers = server.peers.lock().unwrap();
                let (peer, rx) = Peer::new(peer_id, addr, None).unwrap();
                peers.insert(peer_id, peer);
                Some(rx)
            } else {
                None
            }
            .unwrap();

            let peer_id = peer_id.unwrap();
            println!("Peer {} registered\n", &peer_id);

            if let Err(err) = message_loop(&server, socket, peer_id, ws_rx).await {
                eprintln!("An error occurred: {}\n", err);
            }

            // If we are reaching this point - the peer has left
            {
                let mut peers = server.peers.lock().unwrap();
                peers.remove(&peer_id);
                println!("Peer {} has left", &peer_id);
                // println!("PEERS: {:?}", peers);

                // TODO
                // 1) Broadcast message to the MCU that the peer has left
                // 2) Cleanup state - ROOM maintenance
            }
        });
    }
}