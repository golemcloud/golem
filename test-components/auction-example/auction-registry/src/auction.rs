use crate::bindings::auction::auction_client::auction_client;
use crate::model::*;
use golem_rust::wasm_rpc::{AgentId, ComponentId};
use std::env;
use std::str::FromStr;

pub fn create(auction: Auction) {
    let component_id = env::var("AUCTION_COMPONENT_ID").expect("AUCTION_COMPONENT_ID not set");
    let component_id: ComponentId =
        ComponentId::from_str(&component_id).expect("Invalid component id");
    let worker_id = AgentId {
        component_id,
        agent_id: format!("auction-{}", auction.auction_id.auction_id),
    };

    let worker = auction_client::Api::custom(&worker_id);
    let wit_auction = auction.into();
    worker.blocking_initialize(&wit_auction);
}

pub fn create_res(auction: Auction) -> auction_client::RunningAuction {
    let component_id = env::var("AUCTION_COMPONENT_ID").expect("AUCTION_COMPONENT_ID not set");
    let component_id: ComponentId =
        ComponentId::from_str(&component_id).expect("Invalid component id");

    let worker_id = AgentId {
        component_id,
        agent_id: format!("auction-{}", auction.auction_id.auction_id),
    };

    let wit_auction = auction.into();
    auction_client::RunningAuction::custom(&worker_id, &wit_auction)
}
