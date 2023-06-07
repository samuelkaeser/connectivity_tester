use rand::{Rng};
use std::{error::Error, str::FromStr, fs, fs::{File, OpenOptions}, io::{Read, Write}, path::Path, collections::HashSet, net::{SocketAddr, Ipv4Addr}, mem};
use discv5::{enr, enr::{CombinedKey, Enr, NodeId, EnrBuilder}, Discv5, Discv5ConfigBuilder};
use std::sync::{Arc};
use serde::{Serialize, Deserialize};
use tokio::time::{timeout, Duration, Instant, sleep};
use parking_lot::RwLock;
use discv5::handler::{HandlerIn, HandlerOut, NodeContact, Handler};
use discv5::{packet::DefaultProtocolId,
    rpc::{Request, Response, RequestBody, ResponseBody, RequestId, ResponseBody::Nodes},
};


#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EnrPlus {
    enr: Enr<CombinedKey>,
    responisve: bool,
}

macro_rules! arc_rw {
    ( $x: expr ) => {
        Arc::new(RwLock::new($x))
    };
}

fn find_node_custom(receiver_enr: discv5::enr::Enr<CombinedKey>, distance_vector:Vec<u64>, listening_address:SocketAddr) -> HashSet<Enr<CombinedKey>> {

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {

        const MAX_RETRIES: u32 = 1000;
        let mut retries = 0;
        let mut _exit_send;
        let mut sender_handler;
        let mut sender_handler_recv;
        let ip: Ipv4Addr= "0.0.0.0".parse().unwrap();

        loop {
            let ip_1 = "0.0.0.0".parse().unwrap();
            let key = CombinedKey::generate_secp256k1();
            let config = Discv5ConfigBuilder::new().build();
            let sender_enr = EnrBuilder::new("v4")
                .ip4(ip_1)
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
                        println!("Exceeded maximum number of retries, returning empty routing table");
                        let mut nodes_collector: HashSet<Enr<CombinedKey>> = HashSet::new();
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
        let find_node_message = Box::new(Request {
            id: RequestId(vec![2]),
            body: RequestBody::FindNode { distances: distance_vector },
        });

        // sender to send the first message then await for the session to be established
        //let clock_start = Instant::now();
        let receiver_node_contact = match NodeContact::try_from_enr(receiver_enr.clone(), discv5::IpMode::Ip4) {
            Ok(node) => node,
            Err(e) => {
                let mut nodes_collector: HashSet<Enr<CombinedKey>> = HashSet::new();
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

pub fn get_routing_table_custom(receiver_enr: discv5::enr::Enr<CombinedKey>, listening_address:SocketAddr) -> Vec<Enr<CombinedKey>>{
    let mut nodes_collector_1: HashSet<Enr<CombinedKey>> = HashSet::new();
        for i in 240..=256{
        let distance_vector:Vec<u64> = vec![i];
        let new_nodes: HashSet<Enr<CombinedKey>> = find_node_custom(receiver_enr.clone(), distance_vector, listening_address);
        nodes_collector_1.extend(new_nodes);
    }
    let routing_table: Vec<Enr<CombinedKey>> = nodes_collector_1.into_iter().collect();
    return routing_table;
}

pub fn first_bootnode_enr()-> Enr<CombinedKey> {
    // let base_64_string = "enr:-Ly4QFPk-cTMxZ3jWTafiNblEZkQIXGF2aVzCIGW0uHp6KaEAvBMoctE8S7YU0qZtuS7By0AA4YMfKoN9ls_GJRccVpFh2F0dG5ldHOI__________-EZXRoMpCC9KcrAgAQIIS2AQAAAAAAgmlkgnY0gmlwhKh3joWJc2VjcDI1NmsxoQKrxz8M1IHwJqRIpDqdVW_U1PeixMW5SfnBD-8idYIQrIhzeW5jbmV0cw-DdGNwgiMog3VkcIIjKA";
    // Teku team's bootnode
		//"enr:-KG4QOtcP9X1FbIMOe17QNMKqDxCpm14jcX5tiOE4_TyMrFqbmhPZHK_ZPG2Gxb1GE2xdtodOfx9-cgvNtxnRyHEmC0ghGV0aDKQ9aX9QgAAAAD__________4JpZIJ2NIJpcIQDE8KdiXNlY3AyNTZrMaEDhpehBDbZjM_L9ek699Y7vhUJ-eAdMyQW_Fil522Y0fODdGNwgiMog3VkcIIjKA",
		//"enr:-KG4QL-eqFoHy0cI31THvtZjpYUu_Jdw_MO7skQRJxY1g5HTN1A0epPCU6vi0gLGUgrzpU-ygeMSS8ewVxDpKfYmxMMGhGV0aDKQtTA_KgAAAAD__________4JpZIJ2NIJpcIQ2_DUbiXNlY3AyNTZrMaED8GJ2vzUqgL6-KD1xalo1CsmY4X1HaDnyl6Y_WayCo9GDdGNwgiMog3VkcIIjKA",
		// Prylab team's bootnodes
		//"enr:-Ku4QImhMc1z8yCiNJ1TyUxdcfNucje3BGwEHzodEZUan8PherEo4sF7pPHPSIB1NNuSg5fZy7qFsjmUKs2ea1Whi0EBh2F0dG5ldHOIAAAAAAAAAACEZXRoMpD1pf1CAAAAAP__________gmlkgnY0gmlwhBLf22SJc2VjcDI1NmsxoQOVphkDqal4QzPMksc5wnpuC3gvSC8AfbFOnZY_On34wIN1ZHCCIyg",
		//"enr:-Ku4QP2xDnEtUXIjzJ_DhlCRN9SN99RYQPJL92TMlSv7U5C1YnYLjwOQHgZIUXw6c-BvRg2Yc2QsZxxoS_pPRVe0yK8Bh2F0dG5ldHOIAAAAAAAAAACEZXRoMpD1pf1CAAAAAP__________gmlkgnY0gmlwhBLf22SJc2VjcDI1NmsxoQMeFF5GrS7UZpAH2Ly84aLK-TyvH-dRo0JM1i8yygH50YN1ZHCCJxA",
		//"enr:-Ku4QPp9z1W4tAO8Ber_NQierYaOStqhDqQdOPY3bB3jDgkjcbk6YrEnVYIiCBbTxuar3CzS528d2iE7TdJsrL-dEKoBh2F0dG5ldHOIAAAAAAAAAACEZXRoMpD1pf1CAAAAAP__________gmlkgnY0gmlwhBLf22SJc2VjcDI1NmsxoQMw5fqqkw2hHC4F5HZZDPsNmPdB1Gi8JPQK7pRc9XHh-oN1ZHCCKvg",
		// Lighthouse team's bootnodes
		//"enr:-Jq4QItoFUuug_n_qbYbU0OY04-np2wT8rUCauOOXNi0H3BWbDj-zbfZb7otA7jZ6flbBpx1LNZK2TDebZ9dEKx84LYBhGV0aDKQtTA_KgEAAAD__________4JpZIJ2NIJpcISsaa0ZiXNlY3AyNTZrMaEDHAD2JKYevx89W0CcFJFiskdcEzkH_Wdv9iW42qLK79ODdWRwgiMo",
		//"enr:-Jq4QN_YBsUOqQsty1OGvYv48PMaiEt1AzGD1NkYQHaxZoTyVGqMYXg0K9c0LPNWC9pkXmggApp8nygYLsQwScwAgfgBhGV0aDKQtTA_KgEAAAD__________4JpZIJ2NIJpcISLosQxiXNlY3AyNTZrMaEDBJj7_dLFACaxBfaI8KZTh_SSJUjhyAyfshimvSqo22WDdWRwgiMo",
		// EF bootnodes
		//"enr:-Ku4QHqVeJ8PPICcWk1vSn_XcSkjOkNiTg6Fmii5j6vUQgvzMc9L1goFnLKgXqBJspJjIsB91LTOleFmyWWrFVATGngBh2F0dG5ldHOIAAAAAAAAAACEZXRoMpC1MD8qAAAAAP__________gmlkgnY0gmlwhAMRHkWJc2VjcDI1NmsxoQKLVXFOhp2uX6jeT0DvvDpPcU8FWMjQdR4wMuORMhpX24N1ZHCCIyg",
		//"enr:-Ku4QG-2_Md3sZIAUebGYT6g0SMskIml77l6yR-M_JXc-UdNHCmHQeOiMLbylPejyJsdAPsTHJyjJB2sYGDLe0dn8uYBh2F0dG5ldHOIAAAAAAAAAACEZXRoMpC1MD8qAAAAAP__________gmlkgnY0gmlwhBLY-NyJc2VjcDI1NmsxoQORcM6e19T1T9gi7jxEZjk_sjVLGFscUNqAY9obgZaxbIN1ZHCCIyg",
		//"enr:-Ku4QPn5eVhcoF1opaFEvg1b6JNFD2rqVkHQ8HApOKK61OIcIXD127bKWgAtbwI7pnxx6cDyk_nI88TrZKQaGMZj0q0Bh2F0dG5ldHOIAAAAAAAAAACEZXRoMpC1MD8qAAAAAP__________gmlkgnY0gmlwhDayLMaJc2VjcDI1NmsxoQK2sBOLGcUb4AwuYzFuAVCaNHA-dy24UuEKkeFNgCVCsIN1ZHCCIyg",
		//"enr:-Ku4QEWzdnVtXc2Q0ZVigfCGggOVB2Vc1ZCPEc6j21NIFLODSJbvNaef1g4PxhPwl_3kax86YPheFUSLXPRs98vvYsoBh2F0dG5ldHOIAAAAAAAAAACEZXRoMpC1MD8qAAAAAP__________gmlkgnY0gmlwhDZBrP2Jc2VjcDI1NmsxoQM6jr8Rb1ktLEsVcKAPa08wCsKUmvoQ8khiOl_SLozf9IN1ZHCCIyg",
		// Nimbus bootnodes
		//"enr:-LK4QA8FfhaAjlb_BXsXxSfiysR7R52Nhi9JBt4F8SPssu8hdE1BXQQEtVDC3qStCW60LSO7hEsVHv5zm8_6Vnjhcn0Bh2F0dG5ldHOIAAAAAAAAAACEZXRoMpC1MD8qAAAAAP__________gmlkgnY0gmlwhAN4aBKJc2VjcDI1NmsxoQJerDhsJ-KxZ8sHySMOCmTO6sHM3iCFQ6VMvLTe948MyYN0Y3CCI4yDdWRwgiOM",
		//"enr:-LK4QKWrXTpV9T78hNG6s8AM6IO4XH9kFT91uZtFg1GcsJ6dKovDOr1jtAAFPnS2lvNltkOGA9k29BUN7lFh_sjuc9QBh2F0dG5ldHOIAAAAAAAAAACEZXRoMpC1MD8qAAAAAP__________gmlkgnY0gmlwhANAdd-Jc2VjcDI1NmsxoQLQa6ai7y9PMN5hpLe5HmiJSlYzMuzP7ZhwRiwHvqNXdoN0Y3CCI4yDdWRwgiOM",
    let enr_1:Enr<CombinedKey> = discv5::enr::Enr::from_str("enr:-Jq4QItoFUuug_n_qbYbU0OY04-np2wT8rUCauOOXNi0H3BWbDj-zbfZb7otA7jZ6flbBpx1LNZK2TDebZ9dEKx84LYBhGV0aDKQtTA_KgEAAAD__________4JpZIJ2NIJpcISsaa0ZiXNlY3AyNTZrMaEDHAD2JKYevx89W0CcFJFiskdcEzkH_Wdv9iW42qLK79ODdWRwgiMo").unwrap();
    return enr_1;
}

pub fn delete_last_character_and_add_closing_bracket(file_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Read the file content
    let mut file = File::open(file_path)?;
    let mut content = String::new();
    file.read_to_string(&mut content)?;

    // Remove the last character from the content
    content.pop();

    // Add the closing bracket
    content.push(']');

    // Overwrite the file with the modified content
    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(file_path)?;
    file.write_all(content.as_bytes())?;

    Ok(())
}

pub fn subtract_one(mut array: [u8; 32]) -> [u8; 32] {
    for i in (0..array.len()).rev() {
        if array[i] == 0 {
            array[i] = 255; // wrap around to 255 if the current element is 0
        } else {
            array[i] -= 1;
            break; // exit the loop as soon as a non-zero element is found
        }
    }
    return array;
}

pub fn array_to_node_id(input_array:[u8; 32]) -> NodeId {
    let node_id: NodeId= match NodeId::try_from(input_array) {
        Ok(id) => {
            Some(id).unwrap()
        }
        Err(e) => {
            eprintln!("Failed to convert: {:?}", e);
            None.unwrap()
        }
    };
    return node_id;
}

pub fn random_node_id_at_distance(distance: [u8; 32], from: &[u8; 32]) -> NodeId {
    let mut rng = rand::thread_rng();

    // Generate a random mask at the specified distance.
    let mut random_bytes = [0u8; 32];
    rng.fill(&mut random_bytes);
    
    let distance_minus_one_array = subtract_one(distance);
    
    let mut mask = [0u8; 32];
    for i in 0..32 {
        mask[i] = distance_minus_one_array[i] & random_bytes[i];
    }
    // Apply the mask and XOR it with the original Node ID.
    let mut result_1_array = [0u8; 32];

    for i in 0..32 {
        result_1_array[i] = distance[i] ^ mask[i];
    }
    
    let from_array = from;
    let mut result_2_array = [0u8; 32];

    for i in 0..32 {
        result_2_array[i] = from_array[i] ^ result_1_array[i];
    }
    let new_node_id = array_to_node_id(result_2_array);
    return new_node_id;
}

pub fn set_bit(index: usize) -> [u8; 32] {
    let mut array = [0u8; 32];
    let mut byte_index = 0;
    let mut bit_index = 0;
    if index != 0{
        byte_index = (index-1) / 8;
        bit_index = (index-1) % 8;
    }
    if byte_index < 32 {
        array[31 - byte_index] |= 1 << bit_index;
    }
    return array;
}

pub async fn find_node_and_handle_error(
    discv5: &mut Discv5,
    target_node: NodeId,
) -> Result<Vec<Enr<CombinedKey>>, discv5::QueryError> {
    //println!("went into find_node_and_handle_error");
    match discv5.find_node(target_node).await {
        Ok(results) => {
            //println!("received good results within find_node_and_handle_error");
            Ok(results)
        }
        Err(e) => {
            eprintln!("Error in find_node: {:?}", e);
            Err(e)
        }
    }
}

async fn is_node_responsive(destination_enr: Enr<CombinedKey>) -> Result<bool, Box<dyn std::error::Error>> {
    let listen_addr = "0.0.0.0:9000".parse::<SocketAddr>().unwrap();
    // construct a local ENR
    let enr_key_server = CombinedKey::generate_secp256k1();
    let enr_server = enr::EnrBuilder::new("v4").build(&enr_key_server).unwrap();
    // default configuration
    let config = Discv5ConfigBuilder::new().build();

    // construct the discv5 server
    let mut discv5: Discv5 = Discv5::new(enr_server, enr_key_server, config).unwrap();
    discv5.add_enr(destination_enr.clone());
    //println!("call is_node_responsive with the following enr: {:?}", destination_enr);
    match discv5.start(listen_addr).await {
        Ok(_) => {
            //println!("Server in is_node_responsive started successfully on {:?}", listen_addr);
        }
        Err(e) => {
            eprintln!("Server in is_node_responsive failed to start on {:?}: {:?}", listen_addr, e);
        }
    }
    // Send a FINDNODE request to the target ENR
    let target_node_id = destination_enr.node_id();
    let start = Instant::now();
    //println!("arrived before find_node_and_handle_error - call");
    let result = find_node_and_handle_error(&mut discv5, target_node_id).await;
    //println!("went past the find_node_and_handle_error - call");
    match &result {
        Ok(_) => {
            // Process the results
            //println!("Processing FindNode results:");
            //for enr in result.as_ref().unwrap() {
            //    println!("{}", enr.to_string())
           // }
           // println!("Found {} new nodes after calling find_node with traget node: {}", result.unwrap().len(), target_node_id.to_string());
        }
        Err(e) => {
            // Handle the error
            eprintln!("Error processing FindNode results: {:?}", e);
        }
    }

    let elapsed_time = start.elapsed();
    // Wrap the Discv5 instance with an Option
    let mut discv5_option = Some(discv5);

    // To stop the server, explicitly drop the discv5 instance and set the Option to None
    mem::drop(discv5_option.take());

    // Check if the instance has been dropped
    if discv5_option.is_none() {
        //println!("Server stopped, port released");
    }else{
        println!("Motherfucking server didn't stop");
    }


    // Check if the response time is more than 1 millisecond
    Ok(elapsed_time > Duration::from_millis(1))
}

pub async fn process_enrs(enrs: Vec<Enr<CombinedKey>>) -> Result<Vec<EnrPlus>, Box<dyn Error>> {
    let mut results = Vec::new();

    for enr in enrs {
        let is_responsive = is_node_responsive(enr.clone()).await?;
        results.push((enr, is_responsive));
    }

    let enr_with_features: Vec<EnrPlus> = results
        .into_iter()
        .map(|(enr, is_responsive)| EnrPlus { enr, responisve: is_responsive })
        .collect();

    Ok(enr_with_features)
}

pub fn filter_new_nodes(new_found:Vec<Enr<CombinedKey>>, current_nodes:&Vec<Enr<CombinedKey>>) -> Vec<Enr<CombinedKey>>{
    let mut new_nodes: Vec<Enr<CombinedKey>> = Vec::new();
    for node in new_found.iter(){
        if !current_nodes.contains(&node){
            new_nodes.push(node.clone());
        }
    }
    return new_nodes;
}

pub fn node_id_to_array(node_id: NodeId) -> [u8; 32] {
    let bytes = node_id.as_ref();
    let mut array = [0u8; 32];
    array.copy_from_slice(bytes);
    return array;
}

fn xor_distance_msb(a: &[u8; 32], b: &[u8; 32]) -> usize {
    let mut distance = 0;
    for i in (0..a.len()).rev() {
        for j in (0..8).rev() {
            let same = ((a[i] >> j)& 1) == ((b[i] >> j)& 1);
            if !same {
                distance = 256 - (8*i + 7 - j);
                break;
            }
        }
    }
    distance
}

pub fn extract_responsive_enrs(enr_with_features: Vec<EnrPlus>) -> Vec<Enr<CombinedKey>> {
    enr_with_features
        .into_iter()
        .filter(|enr_with_feature| enr_with_feature.responisve)
        .map(|enr_with_feature| enr_with_feature.enr)
        .collect()
}

pub fn save_enrs_to_file(enrs: &Vec<EnrPlus>, file: &str) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .append(true) // Append to the file instead of truncating
        .open(file)?;

    for (index, enr) in enrs.iter().enumerate() {
        let json_data = serde_json::to_string(&enr)?;
        file.write_all(json_data.as_bytes())?;
        file.write_all(b",")?;
    }
    Ok(())
}

pub fn save_routing_table_to_file(enrs: &Vec<Enr<CombinedKey>>, file: &str) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .append(true) // Append to the file instead of truncating
        .open(file)?;

    for enr in enrs {
        writeln!(file, "{}", enr.to_base64())?;
    }
    Ok(())
}

pub fn determine_furthest_node_distance(new_found_nodes: &HashSet<Enr<CombinedKey>>, destination_node_enr: &Enr<CombinedKey>) -> usize {
    let mut max_distance = 0;
    let mut furthest_node : Enr<CombinedKey> = destination_node_enr.clone();
    for node in new_found_nodes{
        let distance = xor_distance_msb(&node_id_to_array(node.node_id()), &node_id_to_array(destination_node_enr.node_id()));
        if distance > max_distance{
            max_distance = distance;
            furthest_node = node.clone();
        }
    }
    //println!("Distance between node {} and node {} is {}, therefore the distance in the next round is {}", furthest_node.node_id().to_string(), destination_node_enr.node_id().to_string(), max_distance, max_distance);
    return max_distance
}

pub fn write_vector_to_file(vec: Vec<usize>, file_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Convert the vector of numbers to a string representation
    let vec_string = vec.iter()
        .map(|num| num.to_string())
        .collect::<Vec<String>>()
        .join("\n");

    // Write the string representation to the file
    let mut file = File::create(file_path)?;
    file.write_all(vec_string.as_bytes())?;

    Ok(())
}

pub fn get_entire_routing_table(
    destination_node_enr: Enr<CombinedKey>, 
    listening_address: SocketAddr,
) -> Vec<Enr<CombinedKey>>{
    
    let mut runtime = tokio::runtime::Builder::new_multi_thread()
       .thread_name("Discv5-example")
       .enable_all()
       .build()
       .unwrap();

    let mut new_found_nodes = HashSet::new();
    let mut done: bool = false;
    let mut distance = 235;
    //let bitstring_distance = set_bit(distance);
    //let target_node_id: NodeId = random_node_id_at_distance(bitstring_distance, &node_id_to_array(destination_node_enr.node_id()));

    while !done {
        // construct a local ENR
        let enr_key = CombinedKey::generate_secp256k1();
        let enr = enr::EnrBuilder::new("v4").build(&enr_key).unwrap();
        // default configuration
        let config = Discv5ConfigBuilder::new().build();

        // construct the discv5 server
        let mut discv5: Discv5 = Discv5::new(enr, enr_key, config).unwrap();
        let result = runtime.block_on(discv5.start(listening_address));

        match result {
            Ok(_) => {
                println!("Server for destination node {} in get_entire_routing_table started successfully on {:?}", destination_node_enr, listening_address);
            }
            Err(e) => {
                eprintln!("Server in get_entire_routing_table failed to start on {:?}: {:?}", listening_address, e);
            }
        }

        //println!("Currently {} nodes in routing table before calling add_enr", discv5.table_entries().len());
        let old_size = new_found_nodes.len();
        discv5.add_enr(destination_node_enr.clone());
        //println!("Currently {} nodes in routing table after calling add_enr", discv5.table_entries().len());

        let bitstring_distance = set_bit(distance);
        let target_node_id: NodeId = random_node_id_at_distance(bitstring_distance, &node_id_to_array(destination_node_enr.node_id()));
        //let mut result: Result<Vec<Enr<CombinedKey>>, discv5::QueryError> = Err(discv5::QueryError::ServiceNotStarted);
        //println!("Destination node: {}", destination_node_enr.node_id().to_string());
        let result = runtime.block_on(find_node_and_handle_error(&mut discv5, target_node_id));

        match &result {
            Ok(_) => {
                // Process the results
                //println!("Processing FindNode results:");
                for enr in result.as_ref().unwrap() {
                    new_found_nodes.insert(enr.clone());
                    //println!("{}", enr.to_string())
                }
                println!("Currently {} nodes found", new_found_nodes.len());
                //println!("Found {} new nodes after calling find_node with distance: {} and traget node: {}", result.unwrap().len(), distance, target_node_id.to_string());
                if old_size == new_found_nodes.len(){
                    if distance == 256{
                        //println!("Didn't find any new nodes and reached the max distance");
                        done = true;
                    }else{
                        //println!("Didn't find any new nodes, increasing distance from {} by one to {}", distance, distance+1);
                        distance += 1;
                    }
                }else{
                    let furthest_distance = determine_furthest_node_distance(&new_found_nodes, &destination_node_enr);
                    if furthest_distance != 0{
                        distance = furthest_distance;
                    }
                }
            }
            Err(e) => {
                // Handle the error
                eprintln!("Error processing FindNode results: {:?}, continuing loop", e);
            }
        };
        // Wrap the Discv5 instance with an Option
        let mut discv5_option = Some(discv5);

        // To stop the server, explicitly drop the discv5 instance and set the Option to None
        mem::drop(discv5_option.take());

        // Check if the instance has been dropped
        if discv5_option.is_none() {
        //println!("Server stopped, port released");
        }else{
        println!("Motherfucking server didn't stop");
        }
    }
    let routing_table: Vec<Enr<CombinedKey>> = new_found_nodes.into_iter().collect();
    return routing_table;
}

pub fn save_to_file<T: ToString>(file_path: &Path, data: &[T]) -> std::io::Result<()> {
    let mut file = fs::File::create(file_path)?;

    for item in data {
        writeln!(file, "{}", item.to_string())?;
    }

    file.flush()?;
    Ok(())
}

// Dummy implementation of process_nodes. Use for testing.
fn process_nodes(node: Enr<CombinedKey>, listening_address:SocketAddr) -> Vec<Enr<CombinedKey>> {
    let mut rng = rand::thread_rng();
    let sleep_time = rng.gen_range(10..20);
    println!("Listening for node {} on {:?}", node, listening_address);
    sleep(Duration::from_secs(sleep_time));

    let number_of_nodes = rng.gen_range(3..11);
    let mut new_nodes = Vec::<Enr<CombinedKey>>::new();
    for _ in 0..(number_of_nodes){
        let enr_key = CombinedKey::generate_secp256k1();
        let enr = enr::EnrBuilder::new("v4").build(&enr_key).unwrap();
        new_nodes.push(enr);
    }

    return new_nodes
}