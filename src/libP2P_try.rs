use libp2p::{
    kad::{
        record::Key, Kademlia, KademliaConfig, KademliaEvent, QueryId, QueryResult, store::MemoryStore,
    },
    identity,
    PeerId,
    Swarm,
    core::upgrade,
    noise::{Keypair, NoiseConfig, X25519Spec, KeypairIdentity},
    yamux::{self, YamuxConfig},
    swarm::SwarmEvent,
};
use futures::prelude::*;
use libp2p::Transport;
use std::str::FromStr;
use discv5::{enr, enr::{CombinedKey, CombinedPublicKey, Enr, NodeId, ed25519_dalek::PUBLIC_KEY_LENGTH}, Discv5};
use libp2p::identity::{PublicKey, ed25519};
use secp256k1::{PublicKey as SecpPublicKey, SecretKey as SecpSecretKey};
use libp2p::core::{PublicKey as CorePublicKey, multiaddr::{Protocol, Multiaddr}};
use std::net::{IpAddr, SocketAddr};
use libsecp256k1::Error as LibsecpError;
use k256::ecdsa::VerifyingKey;
use discv5::enr::k256::elliptic_curve::sec1::ToEncodedPoint;
use tokio::net::tcp;
use enr::secp256k1;
const UNCOMPRESSED_PUBLIC_KEY_SIZE: usize = 65;

fn enr_to_multiaddr(enr: &Enr<CombinedKey>) -> Option<Multiaddr> {
    let enr_public_key = enr.public_key();
    let core_key = match enr_public_key {
        CombinedPublicKey::Secp256k1(secp_key) => {
            let raw_secp_key = secp_key.as_ref();
            let k256_verifying_key = VerifyingKey::from_sec1_bytes(raw_secp_key).ok()?;
            let secp256k1_public_key = secp256k1::PublicKey::from_bytes(k256_verifying_key.to_bytes().as_ref()).ok()?;
            PublicKey::Secp256k1(secp256k1_public_key)
        }
        CombinedPublicKey::Ed25519(ed25519_key) => {
            let raw_ed25519_key = ed25519_key.as_bytes();
            PublicKey::Ed25519(libp2p::identity::ed25519::PublicKey::decode(raw_ed25519_key).unwrap())
        }
    };

    let enr_peer_id = PeerId::from(core_key);
    let ip: IpAddr = IpAddr::V4(enr.ip4().unwrap());
    let port: u16 = enr.udp4().unwrap_or(0);
    let socket_addr = SocketAddr::new(ip, port);

    let mut ma = Multiaddr::empty();
    match socket_addr {
        SocketAddr::V4(socket_addr_v4) => {
            ma.push(Protocol::Ip4(socket_addr_v4.ip().clone()));
            ma.push(Protocol::Tcp(socket_addr_v4.port()));
        }
        SocketAddr::V6(socket_addr_v6) => {
            ma.push(Protocol::Ip6(socket_addr_v6.ip().clone()));
            ma.push(Protocol::Tcp(socket_addr_v6.port()));
        }
    }

    let enr_multiaddr_with_peer_id = ma.with(libp2p::multiaddr::Protocol::P2p(enr_peer_id.into()));
    Some(enr_multiaddr_with_peer_id)
}

fn main() {
    // Create an identity Keypair for this node
    let local_key = identity::Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(local_key.public());
    println!("Local peer id: {:?}", local_peer_id);

    // Set up an encrypted TCP Transport
    let noise_keys = Keypair::<X25519Spec>::new().into_authentic(&local_key).unwrap();
    let noise = NoiseConfig::xx(noise_keys).into_authenticated();
    let yamux_config = YamuxConfig::default();
    let transport = tcp::TcpConfig::new()
        .upgrade(upgrade::Version::V1)
        .authenticate(noise)
        .multiplex(yamux_config)
        .boxed();

    let mut cfg = KademliaConfig::default();
    cfg.set_protocol_name(std::borrow::Cow::Owned(b"/custom/eth2-kad/1.0.0".to_vec()));

    let store = MemoryStore::new(local_peer_id.clone());
    let mut kademlia = Kademlia::with_config(local_peer_id.clone(), store, cfg);

    let target_peer_id: PeerId = "QmQZK9UxbBvU8W6ScdXC6jKq3HKPXp6fm8H6Ksa1AmftQi".parse().unwrap();

    let enr_str = "enr:-Jq4QItoFUuug_n_qbYbU0OY04-np2wT8rUCauOOXNi0H3BWbDj-zbfZb7otA7jZ6flbBpx1LNZK2TDebZ9dEKx84LYBhGV0aDKQtTA_KgEAAAD__________4JpZIJ2NIJpcISsaa0ZiXNlY3AyNTZrMaEDHAD2JKYevx89W0CcFJFiskdcEzkH_Wdv9iW42qLK79ODdWRwgiMo";
    let destination_enr: Enr<CombinedKey> = Enr::from_str(enr_str).unwrap();
    

    println!("ENR Multiaddr: {:?}", enr_multiaddr_with_peer_id);
    let target_key = Key::new(&target_peer_id.to_base58());

    kademlia.start_providing(target_key.clone()).unwrap();

    let mut swarm: Swarm<Kademlia<MemoryStore>> = Swarm::new(transport, kademlia, local_peer_id);

    let addr: Multiaddr = "/ip4/127.0.0.1/tcp/4001".parse().unwrap();
    Swarm::listen_on(&mut swarm, addr).unwrap();
    Swarm::dial_addr(&mut swarm, enr_multiaddr_with_peer_id).unwrap();

    let query_id = swarm.behaviour_mut().get_closest_peers(target_key.to_vec());

    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            let mut query_finished = false;

            while let Some(event) = swarm.next().await {
                match event {
                    SwarmEvent::Behaviour(KademliaEvent::OutboundQueryCompleted { id, result, .. }) => {
                        if id == query_id {
                            println!("Find node query finished:");
                            println!("This is the result: {:?}", result);
                            match result {
                                QueryResult::GetClosestPeers(res) => {
                                    match res {
                                        Ok(ok_res) => {
                                            for peer in ok_res.peers {
                                                println!("  Peer: {:?}", peer);
                                            }
                                        },
                                        Err(err) => {
                                            println!("Error in get_closest_peers response: {:?}", err);
                                        }
                                    }
                                },
                                _ => (),
                            }
                            query_finished = true;
                        }
                    },
                    _ => (),
                }
            
                if query_finished {
                    break;
                }
            }
            
            
        });
}
