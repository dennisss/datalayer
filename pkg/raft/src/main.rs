#![feature(proc_macro_hygiene, decl_macro, type_alias_enum_variants, generators)]

#[macro_use] extern crate serde_derive;
#[macro_use] extern crate error_chain;

extern crate futures_await as futures;

extern crate rand;
extern crate serde;
extern crate rmp_serde as rmps;
extern crate hyper;
extern crate tokio;
extern crate clap;
extern crate bytes;
extern crate raft;
extern crate core;


mod redis;
mod key_value;

use raft::errors::*;
use raft::protos::*;
use raft::state_machine::*;
use raft::log::*;
use raft::server::{Server, ServerInitialState};
use raft::atomic::*;
use raft::rpc::{Client, marshal, unmarshal};
use raft::server_protos::*;
use raft::simple_log::*;
use raft::discovery::DiscoveryService;
use raft::routing::*;
use std::path::Path;
use clap::{Arg, App};
use std::sync::{Arc, Mutex};
use futures::future::*;
use core::DirLock;
use rand::prelude::*;
use futures::prelude::*;
use futures::prelude::await;

use redis::resp::*;
use key_value::*;


/*
	Some form of client interface is needed so that we can forward arbitrary entries to any server

*/

/*
let mut config = Configuration {
	last_applied: 0, // TODO: Convert to an Option
	members: HashSet::new(),
	learners: HashSet::new()
};

config.members.insert(ServerDescriptor {
	id: 1,
	addr: "http://127.0.0.1:4001".to_string()
});

config.members.insert(ServerDescriptor {
	id: 2,
	addr: "http://127.0.0.1:4002".to_string()
});
*/

// XXX: See https://github.com/etcd-io/etcd/blob/fa92397e182286125c72bf52d95f9f496f733bdf/raft/raft.go#L113 for more useful config parameters


/*
	In order to make a server, we must at least have a server id 
	- First and for-most, if there already exists a file on disk with metadata, then we should use that
	- Otherwise, we must just block until we have a machine id by some other method
		- If an existing cluster exists, then we will ask it to make a new cluster id
		- Otherwise, the main() script must wait for someone to bootstrap us and give ourselves id 1
*/


/*
	Other scenarios
	- Server startup
		- Server always starts completely idle and in a mode that would reject external requests
		- If we have configuration on disk already, then we can use that
		- If we start with a join cli flag, then we can:
			- Ask the cluster to create a new unique machine id (we could trivially use an empty log entry and commit that to create a new id) <- Must make sure this does not conflict with the master's id if we make many servers before writing other data
	
		- If we are sent a one-time init packet via http post, then we will start a new cluster on ourselves
*/

/*
	Summary of event variables:
	- OnCommited
		- Ideally this would be a channel tht can pass the Arc references to the listeners so that maybe we don't need to relock in order to take things out of the log
		- ^ This will be consumed by clients waiting on proposals to be written and by the state machine thread waiting for the state machine to get fully applied 
	- OnApplied
		- Waiting for when a change is applied to the state machine
	- OnWritten
		- Waiting for when a set of log entries have been persisted to the log file
	- OnStateChange
		- Mainly to wake up the cycling thread so that it can 
		- ^ This will always only have a single consumer so this may always be held as light weight as possibl


	TODO: Future optimization would be to also save the metadata into the log file so that we are only ever writing to one append-only file all the time
		- I think this is how etcd implements it as well
*/


use raft::rpc::ServerService;
use raft::rpc::*;

struct RaftRedisServer {
	server: Arc<Server<KeyValueReturn>>,
	state_machine: Arc<MemoryKVStateMachine>
}


use redis::server::CommandResponse;
use redis::resp::RESPString;

impl redis::server::Service for RaftRedisServer {

	fn get(&self, key: RESPString) -> CommandResponse {
		let state_machine = &self.state_machine;

		let val = state_machine.get(key.as_ref());

		Box::new(ok(match val {
			Some(v) => RESPObject::BulkString(v), // NOTE: THis implies that we have no efficient way to serialize from references anyway
			None => RESPObject::Nil
		}))
	}

	fn set(&self, key: RESPString, value: RESPString) -> CommandResponse {
		let state_machine = &self.state_machine;
		let server = &self.server;

		let op = KeyValueOperation::Set {
			key: key.as_ref().to_vec(),
			value: value.as_ref().to_vec(),
			expires: None,
			compare: None
		};

		// XXX: If they are owned, it is better to 
		let op_data = marshal(op).unwrap();

		Box::new(server.execute(op_data)
		.map_err(|e| {
			eprintln!("SET failed with {:?}", e);
			Error::from("Failed")
		})
		.map(|res| {
			RESPObject::SimpleString(b"OK"[..].into())
		}))

		/*
		Box::new(server.propose(raft::protos::ProposeRequest {
			data: LogEntryData::Command(op_data),
			wait: true
		})
		.map(|_| {
			RESPObject::SimpleString(b"OK"[..].into())
		}))
		*/
	}

	fn del(&self, key: RESPString) -> CommandResponse {
		// TODO: This requires knowledge of how many keys were actually deleted (for the case of non-existent keys)

		let state_machine = &self.state_machine;
		let server = &self.server;

		let op = KeyValueOperation::Delete {
			key: key.as_ref().to_vec()
		};

		// XXX: If they are owned, it is better to 
		let op_data = marshal(op).unwrap();

		Box::new(server.execute(op_data)
		.map_err(|e| {
			eprintln!("DEL failed with {:?}", e);
			Error::from("Failed")
		})
		.map(|res| {
			RESPObject::Integer(if res.success { 1 } else { 0 })
		}))
		
		/*
		Box::new(server.propose(raft::protos::ProposeRequest {
			data: LogEntryData::Command(op_data),
			wait: true
		})
		.map(|_| {
			RESPObject::Integer(1)
		}))*/
	}

	fn publish(&self, channel: RESPString, object: RESPObject) -> Box<Future<Item=usize, Error=Error> + Send> {
		Box::new(ok(0))
	}

	fn subscribe(&self, channel: RESPString) -> Box<Future<Item=(), Error=Error> + Send> {
		Box::new(ok(()))
	}

	fn unsubscribe(&self, channel: RESPString) -> Box<Future<Item=(), Error=Error> + Send> {
		Box::new(ok(()))
	}
}

/*
	The general idea

	- We will expose a single DiscoveryService
		- We can't register ourselves 
		- 

	-> 

	XXX: DiscoveryService will end up requesting ourselves in the case of starting up the services themselves starting up
	-> Should be ideally topology agnostic
	-> We only NEED to do a discovery if we are not 

	-> We always want to have a discovery service
		-> 


	When do we HAVE to wait for initial discovery:
	-> Only if we 

	So the gist:
		-> Can't create disk 

	-> Every single server if given a seed list should try to reach that seed list on startup just to try and get itself in the cluster
		-> Naturally in the case of a bootstrap

	-> In most cases, if 

	-> So yes, I think that we can still have a gossip protocol generalized to some form of identity value


	General startup steps:
	1. Load existing data
	2. If new and not bootstrap, do a seed round
	3. If new and not bootstrap, generate a new id via a proposal
	4. If new, save our data to disk
	5. Start up our server
		- This should setup our local identity
		- NOTE: Before this point, our agent should not have an identity, but may have a cluster id
	6. If not part of the cluster, add ourselves 
	6. Start gossiper
		- We must git the gossip list another time to tell the cluster where we are
		-> Sometimes will occur as part of the AddMember process (but not always if we aren't )
		- Now that we have an identity, we must 
		- The other notition is to thing of ourselves as always having an identity (in)

*/

#[async]
fn main_task() -> Result<()> {
	let matches = App::new("Raft")
		.about("Sample consensus reaching node")
		.arg(Arg::with_name("dir")
			.long("dir")
			.short("d")
			.value_name("DIRECTORY_PATH")
			.help("An existing directory to store data file for this unique instance")
			.required(true)
			.takes_value(true))
		// TODO: Also support specifying our rpc listening port
		.arg(Arg::with_name("join")
			.long("join")
			.short("j")
			.value_name("SERVER_ADDRESS")
			.help("Address of a running server to be used for joining its cluster if this instance has not been initialized yet")
			.takes_value(true))
		.arg(Arg::with_name("bootstrap")
			.long("bootstrap")
			.help("Indicates that this should be created as the first node in the cluster"))
		.get_matches();


	// TODO: For now, we will assume that bootstrapping is well known up front although eventually to enforce that it only ever occurs exactly once, we may want to have an admin externally fire exactly one request to trigger it
	// But even if we do pass in bootstrap as an argument, it is still guranteed to bootstrap only once on this machine as we will persistent the bootstrapped configuration before talking to other servers in the cluster

	let dir = Path::new(matches.value_of("dir").unwrap()).to_owned();
	let bootstrap = matches.is_present("bootstrap");
	let seed_list: Vec<String> = vec![
		"http://127.0.0.1:4001".into(),
		"http://127.0.0.1:4002".into()
	];


	let lock = DirLock::open(&dir)?;

	// Ideally an agent would encapsulate saving itself to disk via some file somewhere
	let agent = Arc::new(Mutex::new( NetworkAgent::new() ));

	let client = Arc::new(Client::new(agent.clone()));
	let discovery = Arc::new(DiscoveryService::new(client.clone(), seed_list));

	

	// Basically need to get a (meta, meta_file, config_snapshot, config_file, log_file)

	let meta_builder = BlobFile::builder(&dir.join("meta".to_string()))?;
	let config_builder = BlobFile::builder(&dir.join("config".to_string()))?;
	let log_path = dir.join("log".to_string());

	let mut is_empty: bool;

	// If a previous instance was started in this directory, restart it
	// NOTE: In this case we will ignore the bootstrap flag
	// TODO: Need good handling of missing files that doesn't involve just deleting everything
	// ^ A known issue is that a bootstrapped node will currently not be able to recover if it hasn't fully flushed its own log through the server process

	let (
		meta, meta_file,
		config_snapshot, config_file,
		log
	) : (
		ServerMetadata, BlobFile,
		ServerConfigurationSnapshot, BlobFile,
		SimpleLog
	) = if meta_builder.exists() || config_builder.exists() {

		let (meta_file, meta_data) = meta_builder.open()?;
		let (config_file, config_data) = config_builder.open()?;

		// TODO: Load from disk
		let mut log = SimpleLog::open(&log_path)?;

		let meta = unmarshal(meta_data)?;
		let config_snapshot = unmarshal(config_data)?;

		is_empty = false;

		(meta, meta_file, config_snapshot, config_file, log)
	}
	// Otherwise we are starting a new server instance
	else {
		// Every single server starts with totally empty versions of everything
		let mut meta = raft::protos::Metadata::default();
		let config_snapshot = ServerConfigurationSnapshot::default();
		let mut log = vec![];


		let mut id: ServerId;
		let mut cluster_id: ClusterId;

		// For the first server in the cluster (assuming no configs are already on disk)
		if bootstrap {

			id = 1;
			is_empty = false;

			// Assign a cluster id to our agent (usually would be retrieved through network discovery if not in bootstrap mode)
			cluster_id = rand::thread_rng().next_u64();

			log.push(LogEntry {
				term: 1,
				index: 1,
				data: LogEntryData::Config(ConfigChange::AddMember(1))
			});
		}
		else {
			// TODO: All of this could be in while loop until we are able to connect to the leader and propose a new message on it

			await!(discovery.seed())?;

			// TODO: Instead pick a random one from our list
			let first_id = agent.lock().unwrap().routes.values().next().unwrap().desc.id;

			let ret = await!(client.call_propose(first_id, &ProposeRequest {
				data: LogEntryData::Noop,
				wait: true
			}))?;

			// TODO: If we get here, we may get a not_leader, in which case, if we don't have information on the leader's identity, then we need to ask everyone we know for a new list of server addrs

			println!("Generated new index {}", ret.index);

			id = ret.index;
			is_empty = true;

			cluster_id = agent.lock().unwrap().cluster_id.clone()
				.expect("No cluster_id obtained during initial cluster connection");

		}

		//  XXX: If we are able to get an id, then 
		let server_meta = ServerMetadata {
			id, cluster_id,
			meta
		};

		// Ideally save the log for the first time right here
		let meta_file = meta_builder.create(&marshal(&server_meta)?)?;
		let config_file = config_builder.create(&marshal(&config_snapshot)?)?;
		let log_file = SimpleLog::create(&log_path)?;

		for e in log {
			log_file.append(e);
		}

		// TODO: The config should get immediately comitted and we should immediately safe it with the right cluster id (otherwise this bootstrap will just result in us being left with a totally empty config right?)
		// ^ Although it doesn't really matter all that much

		(
			server_meta, meta_file,
			config_snapshot, config_file,
			log_file
		)
	};

	println!("Starting with id {}", meta.id);

	let state_machine = Arc::new(MemoryKVStateMachine::new());

	let initial_state = ServerInitialState {
		meta, meta_file,
		config_snapshot, config_file,
		log: Box::new(log),
		state_machine: state_machine.clone(),
		last_applied: 0
	};

	println!("COMMIT INDEX {}", initial_state.meta.meta.commit_index);

	let server = Arc::new(Server::new(client.clone(), initial_state));

	// TODO: Support passing in a port (and maybe also an addr)
	let task = Server::start(server.clone());


	// TODO: If one node joins another cluster with one node, does the old leader of that cluster need to step down?

	// THe simpler way to think of this is (if not bootstrap mode and there are zero )
	// But yeah, if we can get rid of the bootstrap caveat, then this i 

	let our_id = client.agent().lock().unwrap().identity.clone().unwrap().id;

	let join_cluster = lazy(move || {

		if !is_empty {
			return err(())
		}

		ok(())
	})
	.and_then(move |_| {

		println!("Planning on joining: ");

		// TODO: Possibly build another layer of client that will do the extra discovery and leader_hint caching


		// For anything to work properly, this must occur after we have an id,

		// XXX: at this point, we should know who the leader is with better precision than this  (based on a leader hint from above)
		client.call_propose(1, &raft::protos::ProposeRequest {
			data: LogEntryData::Config(ConfigChange::AddMember(our_id)),
			wait: false
		}).then(|res| -> FutureResult<(), ()> {

			println!("call_propose response: {:?}", res);
			
			ok(())
		})
		
	})
	.then(|_| {
		ok(())
	});


	let client_server = Arc::new(redis::server::Server::new(RaftRedisServer {
		server: server.clone(), state_machine: state_machine.clone()
	}));

	let client_task = redis::server::Server::start(client_server.clone(), (5000 + our_id) as u16);



	// Run everything
	await!(
		task
		.join(join_cluster)
		.join(client_task)
		.join(DiscoveryService::run(discovery.clone()))
	);


	Ok(())
}


fn main() -> Result<()> {

	tokio::run(lazy(|| {
		main_task()
		.map_err(|e| {
			eprintln!("{:?}", e);
			()
		})
	}));

	// This is where we would perform anything needed to manage regular client requests (and utilize the server handle to perform operations)
	// Noteably we want to respond to clients with nice responses telling them specifically if we are not the actual leader and can't actually fulfill their requests

	Ok(())
}

