//! Blocks are the subunits of each ship in PSO. Chances are, if you're running a
//! private server, you don't need more than one block per ship. But we'll
//! support having as many as you want.

use std::sync::mpsc::Receiver;
use std::sync::mpsc::channel;
use std::collections::HashMap;
use std::rc::Rc;
use std::cell::RefCell;
use std::net::SocketAddr;
use std::sync::Arc;
use std::thread;

use mio::Sender;
use mio::tcp::TcpListener;

use rand::random;

use psomsg::bb::*;

use ::shipgate::client::SgSender;
use ::services::message::NetMsg;
use ::shipgate::client::callbacks::SgCbMgr;
use ::services::{ServiceMsg, Service, ServiceType};
use ::loop_handler::LoopMsg;

pub mod client;
pub mod handler;
pub mod lobbyhandler;

use self::handler::BlockHandler;
use self::client::ClientState;
use self::lobbyhandler::Lobby;

pub struct BlockService {
    receiver: Receiver<ServiceMsg>,
    sender: Sender<LoopMsg>,
    sg_sender: SgCbMgr<BlockHandler>,
    clients: Rc<RefCell<HashMap<usize, Rc<RefCell<ClientState>>>>>,
    lobbies: Rc<RefCell<Vec<Lobby>>>,
    block_num: u16,
    event: u16
}

impl BlockService {
    pub fn spawn(bind: &SocketAddr,
                 sender: Sender<LoopMsg>,
                 sg_sender: &SgSender,
                 key_table: Arc<Vec<u32>>,
                 block_num: u16,
                 event: u16) -> Service {
        let (tx, rx) = channel();

        let listener = TcpListener::bind(bind).expect("Couldn't create tcplistener");

        let sg_sender = sg_sender.clone_with(tx.clone());

        thread::spawn(move|| {
            let d = BlockService {
                receiver: rx,
                sender: sender,
                sg_sender: sg_sender.into(),
                clients: Default::default(),
                lobbies: Default::default(),
                block_num: block_num,
                event: event
            };
            d.run();
        });

        Service::new(listener, tx, ServiceType::Bb(key_table))
    }

    fn make_handler(&self, client_id: usize) -> BlockHandler {
        BlockHandler::new(
            self.sender.clone(),
            self.sg_sender.clone(),
            client_id,
            self.clients.clone(),
            self.lobbies.clone()
        )
    }

    fn init_lobbies(&mut self) {
        let ref mut l = self.lobbies.borrow_mut();
        for i in 0..15 {
            let lobby = Lobby::new(i, self.block_num, self.event);
            l.push(lobby);
        }
        info!("Initialized 15 lobbies with event {}", self.event);
    }

    pub fn run(mut self) {
        info!("Block service running");

        // Initialize lobbies
        self.init_lobbies();

        loop {
            let msg = match self.receiver.recv() {
                Ok(m) => m,
                Err(_) => return
            };

            match msg {
                ServiceMsg::ClientConnected(id) => {
                    info!("Client {} connected to block", id);
                    let sk = vec![random(); 48];
                    let ck = vec![random(); 48];
                    self.sender.send((id, Message::BbWelcome(0, BbWelcome(sk, ck))).into()).unwrap();

                    // Add to clients table
                    let cs = Rc::new(RefCell::new(ClientState::default()));
                    {
                        let ref mut borrow = cs.borrow_mut();
                        borrow.connection_id = id;
                    }
                    {self.clients.borrow_mut().insert(id, cs);}
                },
                ServiceMsg::ClientDisconnected(id) => {
                    info!("Client {} disconnected from block", id);

                    let mut h = self.make_handler(id);

                    // First, we need to check if they're in a lobby or party.
                    {
                        let lr = self.lobbies.clone();
                        let ref mut lobbies = lr.borrow_mut();
                        for l in lobbies.iter_mut() {
                            if l.has_player(id) {
                                l.remove_player(&mut h, id).unwrap();
                                break
                            }
                        }
                    }

                    drop(h);

                    {self.clients.borrow_mut().remove(&id);}
                },
                ServiceMsg::ClientSaid(id, NetMsg::Bb(m)) => {
                    let mut h = self.make_handler(id);
                    match m {
                        Message::BbLogin(_, m) => { h.bb_login(m) },
                        Message::BbCharDat(_, m) => { h.bb_char_dat(m) },
                        Message::BbChat(_, m) => { h.bb_chat(m) },
                        Message::BbCreateGame(_, m) => { h.bb_create_game(m) },
                        Message::BbSubCmd60(_, m) => { h.bb_subcmd_60(m) },
                        Message::LobbyChange(_, m) => { h.bb_lobby_change(m) },
                        a => {
                            info!("{:?}", a);
                        }
                    }
                },
                ServiceMsg::ShipGateMsg(m) => {
                    let req = m.get_response_key();
                    debug!("Shipgate Request {}: Response received", req);
                    let cb;
                    {
                        cb = self.sg_sender.cb_for_req(req)
                    }

                    match cb {
                        Some((client, mut c)) => c(self.make_handler(client), m),
                        None => warn!("Got a SG request response for an unexpected request ID {}.", req)
                    }
                }
                _ => unreachable!()
            }
        }
    }
}
