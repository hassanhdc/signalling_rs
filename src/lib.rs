use anyhow::{anyhow, bail, Context, Result};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio_tungstenite::tungstenite;
use tungstenite::Message as WsMessage;

use futures::channel::mpsc::{self, UnboundedReceiver};

// I have not gotten around to implementing peer messages as enum variants
// which are deserialized and serialized as JSON messages

// use serde_derive::{Deserialize, Serialize};
// #[derive(Serialize, Deserialize)]
// #[serde(rename_all = "lowercase")]
// enum SocketMsg {
//     RoomPeer {
//         // Messages starting wtih ROOM_PEER. Do not warrant the any action
//         // from the Socket Server other than routing the message itself
//         //
//         // TYPES:
//         // 1) ROOM_PEER_MSG - is either an SDP or ICE
//         // 2) ROOM_PEER_JOINED - notification that a peer has joined the ROOM
//         // 3) ROOM_PEER_LEFT - notification that a peer has left the ROOM
//         #[serde(rename = "type")]
//         type_: String,
//         to: u32,
//         from: u32,
//     },
//     Server {
//         // Messages meant specifically for/from the "MCU". Typically conveying
//         // some communication prompting the "MCU" to take some action/measure
//         //
//         // TYPES:
//         // 1) SERVER_RESTART - indicating that all peers from a particular ROOM have left
//         // 2) SERVER_CREATE - notify MCU to ready a new socket to serve a new ROOM session
//         // 3) SERVER_INVITE - notify the MCU to join a particular ROOM session
//         // ... Can be added to
//         #[serde(rename = "type")]
//         type_: String,
//         to: u32,
//         from: u32,
//     },
//     RoomCommand {
//         // Messages meant for the "Socket Server" for ROOM creation
//         room_id: u32,
//         to: u32,
//         from: u32,
//     },
// }

#[derive(Debug, Clone)]
pub struct Peer(Arc<PeerInner>);

#[derive(Debug)]
pub struct PeerInner {
    id: u32,
    addr: SocketAddr,
    status: Mutex<Option<u32>>,
    tx: Arc<Mutex<mpsc::UnboundedSender<WsMessage>>>,
}

#[derive(Debug, Clone)]
pub struct Server(Arc<ServerInner>);

#[derive(Debug)]
pub struct ServerInner {
    //  peers: {uid: [Peer tx,
    //                remote_address,
    //                <"room_id"|None>]}
    pub peers: Mutex<HashMap<u32, Peer>>,
    //
    //
    // rooms: {room_id: [peer1_id, peer2_id, peer3_id, ...]}
    pub rooms: Mutex<HashMap<u32, Vec<u32>>>,
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
    pub fn new(
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
    pub fn new() -> Result<Self> {
        // let (ws_msg_tx, ws_msg_rx) = mpsc::unbounded::<WsMessage>();

        let server = Server(Arc::new(ServerInner {
            peers: Mutex::new(HashMap::new()),
            rooms: Mutex::new(HashMap::new()),
            room_servers: Mutex::new(HashMap::new()),
        }));

        Ok(server)
    }
    pub fn handle_message(&self, message: &str, from_id: u32) -> Result<()> {
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
            let rooms = self.rooms.lock().unwrap();

            // Ensure peer.status == None
            let peer = peers.get(&from_id).unwrap().to_owned();
            if let None = *peer.status.lock().unwrap() {
                bail!("Peer {} is not in a ROOM", from_id)
            }

            let (peer, resp) = if let None = peers.get(&to_id) {
                // If peer with "to_id" is not connected, alter the message and return the same peer
                (
                    peers
                        .get(&from_id)
                        .ok_or_else(|| anyhow!("Cannot find peer {}", from_id))?
                        .to_owned(),
                    format!("ERROR peer {} not found", to_id),
                )
            } else {
                // If peer with "to_id" is connected, message remains same and the return "to_id" peer
                let room_id = peers
                    .get(&from_id)
                    .ok_or_else(|| anyhow!("Cannot find peer {}", from_id))?
                    .status
                    .lock()
                    .unwrap()
                    .unwrap();

                rooms
                    .get(&room_id)
                    .unwrap()
                    .contains(&to_id)
                    .then(|| ())
                    .map_or_else(
                        || {
                            (
                                peers
                                    .get(&from_id)
                                    .ok_or_else(|| anyhow!("Cannot find peer {}", from_id))
                                    .unwrap()
                                    .to_owned(),
                                format!("ERROR peer {} not present in room {}", to_id, room_id),
                            )
                        },
                        |_| {
                            (
                                peers
                                    .get(&to_id)
                                    .ok_or_else(|| anyhow!("Cannot find peer {}", to_id))
                                    .unwrap()
                                    .to_owned(),
                                msg.to_string(),
                            )
                        },
                    )
            };
            drop(peers);
            drop(rooms);

            // Access the channel for the returned peer to we can forward them the message
            let tx = &peer.tx.lock().unwrap();
            tx.unbounded_send(WsMessage::Text(resp.into()))
                .with_context(|| format!("Failed to message on channel"))?;
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

            let peers = self.peers.lock().unwrap();
            let mut rooms = self.rooms.lock().unwrap();

            // We are now handling the CMD_ROOM 'CREATE' and 'JOIN' cases within the same branch for brevity and conciseness
            // since we will be checking/modifying related state, why not group the code at one place

            let peer = peers.get(&from_id).unwrap().to_owned();

            // Ensure peer.status == None
            if let Some(status) = *peer.status.lock().unwrap() {
                bail!(
                    "Peer {} is giving CMD_ROOM_{} but is already in ROOM {}",
                    from_id,
                    command,
                    status
                );
            }

            // Conditionally create the the response for the peer
            let resp = if command == "CREATE" {
                println!("{}: CREATE ROOM {}", from_id, room_id);

                if let None = rooms.get(&room_id) {
                    // If the room_id is unique, create the ROOM and enter the peer into the ROOM
                    rooms.insert(room_id, vec![from_id]);

                    // Change the peer state
                    let peer = peers.get(&from_id).unwrap().to_owned();
                    *peer.status.lock().unwrap() = Some(room_id);
                    drop(peer);

                    // ROOM created, we're good
                    println!("ROOM {} created", room_id);

                    format!("ROOM_OK")
                } else {
                    // The given room_id is not unique
                    println!("ROOM {} already exists", room_id);

                    format!("ERROR ROOM {} already exists", room_id)
                }
                // Debug
                // println!("\nPEERS: {:?}\nROOMS:{:?}\n", peers, rooms);
            } else if command == "JOIN" {
                println!("{}: JOIN ROOM {}", from_id, room_id);

                if let Some(_) = rooms.get(&room_id) {
                    // If the room_id exists, enter the peer into the rooms hashmap
                    rooms.get_mut(&room_id).unwrap().push(from_id);

                    // Change the peer status
                    let peer = peers.get(&from_id).unwrap().to_owned();
                    *peer.status.lock().unwrap() = Some(room_id);
                    drop(peer);

                    // ROOM joined, we're good
                    println!("ROOM {} exists, joining..", room_id);

                    format!("ROOM_OK")
                } else {
                    // ROOM not present for the given room_id
                    println!("ROOM {} does not exist", room_id);

                    format!("ERROR ROOM {} does not exist", room_id)
                }
                // Debug
                // println!("\nPEERS: {:?}\nROOMS:{:?}\n", peers, rooms);
            } else {
                // CMD received was neither "CREATE" nor "JOIN"
                bail!("Received invalid command message: {}", command)
            };
            drop(peers);
            drop(rooms);

            // Forward the response we created to the peer's channel
            let tx = &peer.tx.lock().unwrap();
            tx.unbounded_send(WsMessage::Text(resp.into()))
                .with_context(|| format!("Failed to message on channel"))?;
        }

        Ok(())
    }
}
