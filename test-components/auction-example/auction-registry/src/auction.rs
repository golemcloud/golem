use std::env;
use crate::bindings::auction::auction_client::auction_client;
use crate::bindings::golem::rpc::types::Uri;
use crate::model::*;

pub fn create(auction: Auction) {
    let component_id = env::var("AUCTION_COMPONENT_ID").expect("AUCTION_COMPONENT_ID not set");
    let uri = Uri {
        value: format!(
            "urn:worker:{component_id}/auction-{}",
            auction.auction_id.auction_id
        ),
    };
    let worker = auction_client::Api::new(&uri);
    let wit_auction = auction.into();
    worker.blocking_initialize(&wit_auction);
}

pub fn create_res(auction: Auction) -> auction_client::RunningAuction {
    let component_id = env::var("AUCTION_COMPONENT_ID").expect("AUCTION_COMPONENT_ID not set");
    let uri = Uri {
        value: format!("urn:worker:{component_id}/auction"),
    };
    let wit_auction = auction.into();
    auction_client::RunningAuction::new(&uri, &wit_auction)
}
