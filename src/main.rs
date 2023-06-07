use std::path::Path;
use std::{fs::OpenOptions, io};
mod helper_functions;
use discv5::handler::{HandlerIn, HandlerOut, NodeContact, Handler};
use discv5::{packet::DefaultProtocolId,
    rpc::{Request, Response, RequestBody, ResponseBody, RequestId, ResponseBody::Nodes},
    Discv5ConfigBuilder,
    enr::{CombinedKey, Enr, NodeId, EnrBuilder}, enr,
};
use libp2p::kad::PutRecordContext;
use std::{str::FromStr, sync::{Arc, Mutex}};
use log::info;
use std::time::Instant;
use parking_lot::RwLock;
use tokio::time::{timeout, Duration, sleep};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufReader, BufRead, Write};
use std::net::SocketAddr;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use simplelog::*;
use tokio::sync::{Semaphore, mpsc};
use std::collections::HashSet;

macro_rules! arc_rw {
    ( $x: expr ) => {
        Arc::new(RwLock::new($x))
    };
}

async fn check_connectivity(receiver_enr: discv5::enr::Enr<CombinedKey>, sender_port:u16) -> bool {
    const MAX_RETRIES: u32 = 10;
    let mut retries = 0;
    let mut _exit_send;
    let sender_handler;
    let mut sender_handler_recv;
    let ipv4: Ipv4Addr= "0.0.0.0".parse().unwrap();
    let ip:IpAddr = "0.0.0.0".parse().unwrap();

    let key1 = CombinedKey::generate_secp256k1();
    let listening_address = {
        let port = sender_port;
        SocketAddr::new(ip.clone(), port)
    };
    let config = Discv5ConfigBuilder::new().build();
    let sender_enr = EnrBuilder::new("v4")
        .ip4(ipv4)
        .udp4(sender_port)
        .build(&key1)
        .unwrap();
    
    loop {
        let ip = "0.0.0.0".parse().unwrap();
        let key = CombinedKey::generate_secp256k1();
        let config = Discv5ConfigBuilder::new().build();
        let sender_enr = EnrBuilder::new("v4")
            .ip4(ip)
            .udp4(listening_address.port())
            .build(&key)
            .unwrap();
    
        match Handler::spawn::<DefaultProtocolId>(
            arc_rw!(sender_enr.clone()),
            arc_rw!(key),
            sender_enr.udp4_socket().unwrap().into(),
            config.clone(),
        )
        .await
        {
            Ok((exit_send, handler, handler_recv)) => {
                _exit_send = exit_send;
                sender_handler = handler;
                sender_handler_recv = handler_recv;
                break;
            }
            Err(e) => {
                println!("An error occurred: {}, retrying address {}", e, listening_address);
                retries += 1;
                if retries > MAX_RETRIES {
                    info!("Exceeded maximum number of retries, returning empty routing table for node: {}", receiver_enr);
                    return false;
                }
                sleep(Duration::from_millis(1000)).await;
            }
        }
    }

    let send_message = Box::new(Request {
        id: RequestId(vec![1]),
        body: RequestBody::Ping { enr_seq: 1 },
    });

    // sender to send the first message then await for the session to be established
    let receiver_node_contact = match NodeContact::try_from_enr(receiver_enr.clone(), discv5::IpMode::Ip4) {
        Ok(node_contact) => node_contact,
        Err(err) => {
            eprintln!("Failed to create NodeContact from ENR: {:?}", err);
            return false;  // or handle the error in a different way
        }
    };
    let clock_start = Instant::now();
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

    loop {
        match timeout(Duration::from_millis(2000) , sender_handler_recv.recv()).await {
            Ok(Some(message)) => {
                info!("Received message at time: {:?} , reset last_message_received", clock_start.elapsed());
                match message {
                    HandlerOut::Established(_, _, _) => {
                        info!("Session established");
                        println!("Session established");

                        return true;
                        }
                    HandlerOut::WhoAreYou(wru_ref) => {
                        info!("Sender received whoareyou packet with ref:{:?}", wru_ref);
                        //println!("Sender sent whoareyou packet with ref:{:?}", wru_ref);

                        let _ = sender_handler.send(HandlerIn::WhoAreYou(wru_ref, Some(receiver_enr.clone())));
                    }
                    HandlerOut::Request(addr, _request) => {
                        //println!("Sender received request:{}", request);
                        // required to send a pong response to establish the session
                        let _ = sender_handler.send(HandlerIn::Response(addr.clone(), Box::new(pong_response.clone())));
                        info!("Sender sent response:{:?}", HandlerIn::Response(addr.clone(), Box::new(pong_response.clone())));
                    }
                    
                    HandlerOut::RequestFailed(_, request_error) => {
                        info!("Request failed: {}", request_error);
                        return false;
                    }
                    HandlerOut::Response(_,response) => {
                        info!("Received response:{}", response);
                        return true;
                    }
                }
            }
            Ok(None) => {
                // The channel has been closed.
                info!("The channel has been closed");
                return false;
            },
            Err(_) => {
                // Timeout expired without receiving a message.
                info!("Timeout expired without receiving a message.");
                println!("Timeout expired without receiving a message.");

                return false;
            }
        }
    }
    //})
}

async fn check_find_nodes(receiver_enr: discv5::enr::Enr<CombinedKey>, sender_port:u16) -> HashSet<Enr<CombinedKey>> {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {

        const MAX_RETRIES: u32 = 10;
        let mut retries = 0;
        let mut _exit_send;
        let sender_handler;
        let mut sender_handler_recv;
        let ip:IpAddr = "0.0.0.0".parse().unwrap();
        let listening_address = {
            let port = sender_port;
            SocketAddr::new(ip.clone(), port)
        };

        loop {
            let ipv4 = "0.0.0.0".parse().unwrap();
            let key = CombinedKey::generate_secp256k1();
            let config = Discv5ConfigBuilder::new().build();
            let sender_enr = EnrBuilder::new("v4")
                .ip4(ipv4)
                .udp4(listening_address.port())
                .build(&key)
                .unwrap();
        
            match Handler::spawn::<DefaultProtocolId>(
                arc_rw!(sender_enr.clone()),
                arc_rw!(key),
                sender_enr.udp4_socket().unwrap().into(),
                config.clone(),
            )
            .await
            {
                Ok((exit_send, handler, handler_recv)) => {
                    //println!("Started listening successfully on address {}", listening_address);
                    _exit_send = exit_send;
                    sender_handler = handler;
                    sender_handler_recv = handler_recv;
                    break;
                }
                Err(e) => {
                    println!("An error occurred: {}, retrying address {}", e, listening_address);
                    retries += 1;
                    if retries > MAX_RETRIES {
                        println!("Exceeded maximum number of retries, returning empty routing table for node: {}", receiver_enr);
                        let nodes_collector: HashSet<Enr<CombinedKey>> = HashSet::new();
                        return nodes_collector;
                    }
                    sleep(Duration::from_millis(1000)).await;
                }
            }
        }

        let send_message = Box::new(Request {
            id: RequestId(vec![1]),
            body: RequestBody::Ping { enr_seq: 1 },
        });
        let distance_vector:Vec<u64> = vec![256];
        let find_node_message = Box::new(Request {
            id: RequestId(vec![2]),
            body: RequestBody::FindNode { distances: distance_vector },
        });

        // sender to send the first message then await for the session to be established
        let clock_start = Instant::now();
        let receiver_node_contact = match NodeContact::try_from_enr(receiver_enr.clone(), discv5::IpMode::Ip4) {
            Ok(node) => node,
            Err(_) => {
                let nodes_collector: HashSet<Enr<CombinedKey>> = HashSet::new();
                return nodes_collector;
            }
        };
        
        let _ = sender_handler.send(HandlerIn::Request(
            receiver_node_contact.clone(),
            send_message.clone(),
        ));

        let pong_response = Response {
            id: RequestId(vec![1]),
            body: ResponseBody::Pong {
                enr_seq: 1,
                ip: ip.into(),
                port: listening_address.port(),
            },
        };
        let mut nodes_collector: HashSet<Enr<CombinedKey>> = HashSet::new();

        loop {
            match timeout(Duration::from_millis(2000) , sender_handler_recv.recv()).await {
                Ok(Some(message)) => {
                    info!("Received message at time: {:?} , reset last_message_received", clock_start.elapsed());
                    match message {
                        HandlerOut::Established(_, _, _) => {
                            info!("Established connection: {:?} after {:?} time", message, clock_start.elapsed());
                            //now the session is established, send the rest of the messages
                            let _ = sender_handler.send(HandlerIn::Request(
                                    receiver_node_contact.clone(),
                                    find_node_message.clone(),
                                ));
                                //println!("Sent message {}", find_node_message);
                            }
                        HandlerOut::WhoAreYou(wru_ref) => {
                            info!("Sender received and sent whoareyou packet with ref:{:?}", wru_ref);
                            let _ = sender_handler.send(HandlerIn::WhoAreYou(wru_ref, Some(receiver_enr.clone())));
                        }
                        HandlerOut::Request(addr, request) => {
                            info!("Sender received request:{}", request);
                            // required to send a pong response to establish the session
                            let _ = sender_handler.send(HandlerIn::Response(addr.clone(), Box::new(pong_response.clone())));
                            println!("Sender sent response:{:?}", HandlerIn::Response(addr.clone(), Box::new(pong_response.clone())));
                        }
                        HandlerOut::Response(_, box_response) => {
                            if let Response { id: _, body: Nodes { total: _, nodes } } = *box_response {
                                // Now you can work with the `nodes` vector.
                                //println!("Nodes: {:?}", nodes);
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
                    return nodes_collector;
                    //break;
                }
            }
        }
    })
}

fn strip_first_and_last(s: &str) -> &str {
    let len = s.len();
    &s[1..len-1]
}

fn read_enrs(path:&Path) -> Result<Vec<Enr<CombinedKey>>, Box<dyn std::error::Error>>{
    let file = File::open(&path)?;
    let reader = io::BufReader::new(file);
    let mut enrs :Vec<Enr<CombinedKey>>= Vec::new();
    for line in reader.lines() {
        let line = line?;
        let line = strip_first_and_last(&line);
        let enr:Enr<CombinedKey> = discv5::enr::Enr::from_str(&line)?;
        enrs.push(enr);
    }
    Ok(enrs)
}

fn write_enrs_to_file(enrs: &Vec<Enr<CombinedKey>>, file_path: &str) -> io::Result<()> {
    let mut file = File::create(file_path)?;

    for enr in enrs {
        let enr_string = enr.to_base64(); // Assuming to_base64 method exists
        writeln!(file, "{}", enr_string)?;
    }

    Ok(())
}


async fn process_enrs_connectivity(enrs: Vec<Enr<CombinedKey>>, listening_ports: Vec<u16>, max_concurrent: usize)-> (Vec<Enr<CombinedKey>>, Vec<Enr<CombinedKey>>) {
    // Initialize our semaphore with the maximum number of concurrent tasks.
    let semaphore = Arc::new(Semaphore::new(max_concurrent));
    // Wrap listening_ports in an Arc<Mutex<_>>
    let listening_ports = Arc::new(Mutex::new(listening_ports));
    // Channels to receive results
    let (tx, mut rx) = mpsc::channel::<(Enr<CombinedKey>, bool, u16)>(max_concurrent);

    // Active and non active nodes
    let mut active_nodes = Vec::new();
    let mut non_active_nodes = Vec::new();

    // Spawn a new thread to run the for-loop
    let listening_ports_clone = Arc::clone(&listening_ports);
    tokio::spawn(async move {
        // Process the ENRs
        for enr in enrs.into_iter() {
            let sem_clone = Arc::clone(&semaphore);

            // Clone the transmitter and semaphore for each task
            let tx = tx.clone();
            // Lock the mutex and pop a listening port
            let mut listening_ports = listening_ports_clone.lock().unwrap();
            let listening_port = listening_ports.pop().unwrap();
            drop(listening_ports); // Explicitly drop the lock here
            println!("starting thread with port: {}", listening_port);
            tokio::spawn(async move {
                // Acquire a permit from the semaphore
                let _permit = sem_clone.acquire().await.unwrap();
                let result = check_connectivity(enr.clone(), listening_port).await;
                tx.send((enr, result, listening_port)).await.unwrap();
            });
        }
        drop(tx);
        println!("Passed for-loop");
    });
    // Post process results
    while let Some((enr, result, listening_port)) = rx.recv().await {
        println!("Received result for {}: {}", enr.clone(), result.clone());
        if result {
            active_nodes.push(enr);
        } else {
            non_active_nodes.push(enr);
        }
        // Add the listening_port back to the listening_ports vector
        let mut listening_ports = listening_ports.lock().unwrap();
        listening_ports.push(listening_port);
    }
    println!("Processed all nodes");
    (active_nodes, non_active_nodes)

}

async fn process_enrs_findnode(enrs: Vec<Enr<CombinedKey>>, listening_ports: Vec<u16>, max_concurrent: usize)-> (Vec<Enr<CombinedKey>>, Vec<Enr<CombinedKey>>) {
    // Initialize our semaphore with the maximum number of concurrent tasks.
    let semaphore = Arc::new(Semaphore::new(max_concurrent));
    // Wrap listening_ports in an Arc<Mutex<_>>
    let listening_ports = Arc::new(Mutex::new(listening_ports));
    // Channels to receive results
    let (tx, mut rx) = mpsc::channel::<(Enr<CombinedKey>, bool, u16)>(max_concurrent);

    // Active and non active nodes
    let mut active_nodes = Vec::new();
    let mut non_active_nodes = Vec::new();

    // Spawn a new thread to run the for-loop
    let listening_ports_clone = Arc::clone(&listening_ports);
    tokio::spawn(async move {
        // Process the ENRs
        for enr in enrs.into_iter() {
            let sem_clone = Arc::clone(&semaphore);

            // Clone the transmitter and semaphore for each task
            let tx = tx.clone();
            // Lock the mutex and pop a listening port
            let mut listening_ports = listening_ports_clone.lock().unwrap();
            let listening_port = listening_ports.pop().unwrap();
            drop(listening_ports); // Explicitly drop the lock here
            println!("starting thread with port: {}", listening_port);
            tokio::spawn(async move {
                // Acquire a permit from the semaphore
                let _permit = sem_clone.acquire().await.unwrap();
                let result_1 = check_find_nodes(enr.clone(), listening_port).await;
                let result_2 = check_find_nodes(enr.clone(), listening_port).await;
                let result = (result_1 == result_2);
                tx.send((enr, result, listening_port)).await.unwrap();
            });
        }
        drop(tx);
        println!("Passed for-loop");
    });
    // Post process results
    while let Some((enr, result, listening_port)) = rx.recv().await {
        println!("Received result for {}: {}", enr.clone(), result.clone());
        if result {
            active_nodes.push(enr);
        } else {
            non_active_nodes.push(enr);
        }
        // Add the listening_port back to the listening_ports vector
        let mut listening_ports = listening_ports.lock().unwrap();
        listening_ports.push(listening_port);
    }
    println!("Processed all nodes");
    (active_nodes, non_active_nodes)

}

#[tokio::main]
async fn main(){
    let listening_ports: Vec<u16> = (10000..=40000).collect();
    let path = Path::new("discovered_nodes.txt");
    println!("Reading ENRs from file...");
    let start = Instant::now();
    let enrs = read_enrs(&path).unwrap();
    println!("Took {:?} to read ENRs", start.elapsed());

    let file = File::create("log.txt").unwrap();
    let _ = WriteLogger::init(LevelFilter::Info, Config::default(), file);

    let max_concurrent = 512;
    let num_splits = 50; // define your number of splits here
    let delay_between_splits = Duration::from_nanos(0); // define your delay here

    let split_size = (enrs.len() / num_splits) + (enrs.len() % num_splits > 0) as usize;
    let enrs_splits = enrs.chunks(split_size);

    let mut active_nodes = Vec::new();
    let mut non_active_nodes = Vec::new();

    for (i, enrs_chunk) in enrs_splits.enumerate() {
        println!("Processing split number {}", i + 1);
        let (split_active_nodes, split_non_active_nodes) = process_enrs_connectivity(
            enrs_chunk.to_vec(), 
            listening_ports.clone(), 
            max_concurrent
        ).await;

        active_nodes.extend(split_active_nodes);
        non_active_nodes.extend(split_non_active_nodes);

        // Delay before processing next split unless it's the last split
        if i < num_splits - 1 {
            tokio::time::sleep(delay_between_splits).await;
        }
    }

    let filepath1 = "active_nodes.txt";
    let filepath2 = "non_active_nodes.txt";
    let _ = write_enrs_to_file(&active_nodes, filepath1);
    let _ = write_enrs_to_file(&non_active_nodes, filepath2);

    let float_size_1 = active_nodes.len() as f64;
    let float_size_2 = (active_nodes.len() + non_active_nodes.len()) as f64;
    let share = float_size_1 / float_size_2;
    let percentage = share * (100 as f64);
    println!("{} nodes are responsive, that's {}% of all nodes", active_nodes.len(), percentage)
}

/* 
    let max_concurrent_tasks = 10; // change to your desired number of concurrent tasks

    let (tx, rx) = channel::unbounded();

    let pool_1 = ThreadPoolBuilder::new().num_threads(max_concurrent_tasks).build().unwrap();

    for enr in enrs{
        let sender_port = listening_ports.pop().unwrap();
        let tx = tx.clone();
        pool_1.spawn(move || {
            println!("Launching thread");    
            let result = check_connectivity(enr.clone(), sender_port);
            tx.send((result, enr)).unwrap();
        });
    }
    
    let mut active_nodes = Vec::new();
    let mut non_active_nodes = Vec::new();

    // collect the results from the channel
    while let Ok((is_active, enr)) = rx.try_recv() {
        println!("recovering results...");
        if is_active {
            active_nodes.push(enr);
        } else {
            non_active_nodes.push(enr);
        }
    }
/*
    let mut active_nodes = Vec::new();
    let mut non_active_nodes = Vec::new();
    let max_concurrent_tasks = 200; // change to your desired number of concurrent tasks

    let results: Vec<_> = stream::iter(enrs.into_iter().map(move |enr| {
        let sender_port = listening_ports.pop().unwrap();
        let enr_clone = enr.clone();
        task::spawn(async move {
            let result = check_connectivity(enr_clone, sender_port).await;
            (result, enr)
        })
    }))
    .buffer_unordered(max_concurrent_tasks)
    .collect()
    .await;
    
    for result in results {
        match result {
            Ok((true, enr)) => active_nodes.push(enr),
            Ok((false, enr)) => non_active_nodes.push(enr),
            Err(e) => if e.is_panic() {
                println!("Task panicked while executing")
            } else {
                println!("Encountered an error while retrieving the results")
            },
        }
    }
    */
    */
