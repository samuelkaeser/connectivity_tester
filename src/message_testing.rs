mod helper_functions;
use std::collections::HashSet;
use std::{sync::Arc, str::FromStr};
use discv5::handler::{HandlerIn, HandlerOut, NodeContact, Handler};
use discv5::{packet::DefaultProtocolId,
    rpc::{Request, Response, RequestBody, ResponseBody, RequestId, ResponseBody::Nodes},
    Discv5ConfigBuilder,
    enr::{EnrBuilder, CombinedKey, Enr},
};
use std::time::Instant;
use parking_lot::RwLock;
use tokio::time::{timeout, Duration, sleep};
use std::fs;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::BufReader;
use std::net::SocketAddr;
use std::net::IpAddr;


fn compare_json_files(file1: &str, file2: &str) -> Result<(), Box<dyn std::error::Error>> {
    let file1 = File::open(file1)?;
    let file2 = File::open(file2)?;
    let reader1 = BufReader::new(file1);
    let reader2 = BufReader::new(file2);

    let vec1: Vec<Enr<CombinedKey>> = serde_json::from_reader(reader1)?;
    let vec2: Vec<Enr<CombinedKey>> = serde_json::from_reader(reader2)?;

    let set1: HashSet<_> = vec1.iter().collect();
    let set2: HashSet<_> = vec2.iter().collect();

    if vec1.len() != set1.len() {
        println!("found_nodes_1.json contains duplicates");
    }

    if vec2.len() != set2.len() {
        println!("found_nodes_2.json contains duplicates");
    }

    let diff1: HashSet<_> = set1.difference(&set2).collect();
    let diff2: HashSet<_> = set2.difference(&set1).collect();

    if set1 == set2 {
        println!("The sets are equal");
    } else {
        println!("Elements in set1, but not in set2: Total: {}", diff1.len());
        println!("Elements in set2, but not in set1: Total: {}", diff2.len());
    }

    Ok(())
}

macro_rules! arc_rw {
    ( $x: expr ) => {
        Arc::new(RwLock::new($x))
    };
}

fn find_node_custom(receiver_enr: discv5::enr::Enr<CombinedKey>, distance_vector:Vec<u64>, sender_port:u16) -> HashSet<Enr<CombinedKey>> {

    let mut rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let ip = "0.0.0.0".parse().unwrap();
        let key1 = CombinedKey::generate_secp256k1();

        let config = Discv5ConfigBuilder::new().build();
        let sender_enr = EnrBuilder::new("v4")
            .ip4(ip)
            .udp4(sender_port)
            .build(&key1)
            .unwrap();
        
        let (_exit_send, sender_handler, mut sender_handler_recv) =
            Handler::spawn::<DefaultProtocolId>(
                arc_rw!(sender_enr.clone()),
                arc_rw!(key1),
                sender_enr.udp4_socket().unwrap().into(),
                config.clone(),
            )
            .await
            .unwrap();

        let send_message = Box::new(Request {
            id: RequestId(vec![1]),
            body: RequestBody::Ping { enr_seq: 1 },
        });
        let find_node_message = Box::new(Request {
            id: RequestId(vec![2]),
            body: RequestBody::FindNode { distances: distance_vector },
        });

        // sender to send the first message then await for the session to be established
        let clock_start = Instant::now();
        let receiver_node_contact: NodeContact = NodeContact::try_from_enr(receiver_enr.clone(), discv5::IpMode::Ip4).unwrap();
        let _ = sender_handler.send(HandlerIn::Request(
            receiver_node_contact.clone(),
            send_message.clone(),
        ));

        let pong_response = Response {
            id: RequestId(vec![1]),
            body: ResponseBody::Pong {
                enr_seq: 1,
                ip: ip.into(),
                port: sender_port,
            },
        };
        let mut nodes_collector: HashSet<Enr<CombinedKey>> = HashSet::new();
        let mut last_message_received = Instant::now();

        loop {
            match timeout(Duration::from_millis(500) , sender_handler_recv.recv()).await {
                Ok(Some(message)) => {
                    last_message_received = Instant::now(); // Reset the timer each time a message is received.
                    //println!("Received message at time: {:?} , reset last_message_received", clock_start.elapsed());
                    match message {
                        HandlerOut::Established(_, _, _) => {
                            //println!("Established connection: {:?} after {:?} time", message, clock_start.elapsed());
                            //now the session is established, send the rest of the messages
                            let _ = sender_handler.send(HandlerIn::Request(
                                    receiver_node_contact.clone(),
                                    find_node_message.clone(),
                                ));
                                //println!("Sent message {}", find_node_message);
                            }
                        HandlerOut::WhoAreYou(wru_ref) => {
                            //println!("Sender received whoareyou packet with ref:{:?}", wru_ref);
                            //println!("Sender sent whoareyou packet with ref:{:?}", wru_ref);

                            let _ = sender_handler.send(HandlerIn::WhoAreYou(wru_ref, Some(receiver_enr.clone())));
                        }
                        HandlerOut::Request(addr, request) => {
                            //println!("Sender received request:{}", request);
                            // required to send a pong response to establish the session
                            let _ = sender_handler.send(HandlerIn::Response(addr.clone(), Box::new(pong_response.clone())));
                            println!("Sender sent response:{:?}", HandlerIn::Response(addr.clone(), Box::new(pong_response.clone())));
                        }
                        HandlerOut::Response(_, box_response) => {
                            if let Response { id: _, body: Nodes { total: _, nodes } } = *box_response {
                                // Now you can work with the `nodes` vector.
                                //println!("Nodes: {:?}", nodes);
                                println!("len of nodes: {}", nodes.len());
                                nodes_collector.extend(nodes);
                            } else {
                                //println!("The body of the response is not of type Nodes: {}", box_response);
                            }
                        }
                        HandlerOut::RequestFailed(_, request_error) => {
                            println!("Request failed: {}", request_error);
                        }
                    }
                }
                Ok(None) => {
                    // The channel has been closed.
                    println!("The channel has been closed");
                },
                Err(_) => {
                    // Timeout expired without receiving a message.
                    //println!("Last message was received more than a second ago, breaking out from receiving loop");
                    println!("Nodes: {:?}", nodes_collector);
                    return nodes_collector;
                    //break;
                }
            }
        }
    })
}

fn get_routing_table_custom(receiver_enr: discv5::enr::Enr<CombinedKey>) -> Vec<Enr<CombinedKey>>{
    let mut nodes_collector_1: HashSet<Enr<CombinedKey>> = HashSet::new();
    let sender_port = 5002;
    let ip_address = IpAddr::from_str("0.0.0.0").unwrap();
    let listening_address = SocketAddr::new(ip_address, sender_port);
    for i in 240..=256{
        println!("{}", i);
        let distance_vector:Vec<u64> = vec![i];
        let new_nodes: HashSet<Enr<CombinedKey>> = find_node_custom(receiver_enr.clone(), distance_vector, sender_port);
        nodes_collector_1.extend(new_nodes);
    }
    let routing_table: Vec<Enr<CombinedKey>> = nodes_collector_1.into_iter().collect();
    return routing_table;
}

fn main(){
    //simple_session_message().await;
    let mut nodes_collector_1: HashSet<Enr<CombinedKey>> = HashSet::new();
    let mut nodes_collector_2: HashSet<Enr<CombinedKey>> = HashSet::new();
    let receiver_enr:Enr<CombinedKey> = discv5::enr::Enr::from_str("enr:-LS4QC0q8zzSXlOXa72v06kJHgsz83AaU3nZpNGLY5SxDdBwdOOznjAT1bz0lUEIFoo0vtPkEkm_9TJYIdneL6Uo8ZCCAceHYXR0bmV0c4gIAAAAAAAAAIRldGgykLuk2pYDAAAA__________-CaWSCdjSCaXCEM1EuoolzZWNwMjU2azGhA6AZT42UEnsHQMfm-mW6b1dPXUuLA6vdc6GgPw5i8tIAg3RjcIIjKIN1ZHCCIyg").unwrap();
    let sender_port = 5002;
    let port = 5003;
    let ip_address = IpAddr::from_str("0.0.0.0").unwrap();
    let listening_address = SocketAddr::new(ip_address, port);


    for i in 240..=256{
        println!("{}", i);
        let distance_vector:Vec<u64> = vec![i];
        let new_nodes: HashSet<Enr<CombinedKey>> = find_node_custom(receiver_enr.clone(), distance_vector, sender_port);
        nodes_collector_1.extend(new_nodes);
    }

    let distance_vector: Vec<u64> = (0..=256).collect();
    let new_nodes: HashSet<Enr<CombinedKey>> = find_node_custom(receiver_enr.clone(), distance_vector, sender_port);
    nodes_collector_2.extend(new_nodes);
    

    //let routing_table = helper_functions::get_entire_routing_table(receiver_enr.clone(), listening_address);

    let file1 = "found_nodes_1.json";
    let file2 = "found_nodes_2.json";
    let file3 = "routing_table.json";
    let _ = fs::write(file1, serde_json::to_string(&nodes_collector_1).unwrap());
    let _ = fs::write(file2, serde_json::to_string(&nodes_collector_2).unwrap());
    //let _ = fs::write(file3, serde_json::to_string(&routing_table).unwrap());
    

    compare_json_files(&file1, &file2);
    //compare_json_files(&file1, &file3);
    //compare_json_files(&file2, &file3);
}