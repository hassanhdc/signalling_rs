use serde_derive::{Deserialize, Serialize};

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
