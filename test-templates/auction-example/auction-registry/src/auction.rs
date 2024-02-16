use std::env;
use crate::bindings::auction::auction_stub::stub_auction;

use crate::model::*;

pub fn create(auction: Auction) {
    let template_id = env::var("AUCTION_TEMPLATE_ID").expect("AUCTION_TEMPLATE_ID not set");
    let uri = stub_auction::Uri {
        value: format!("worker://{template_id}/auction-{}", auction.auction_id.auction_id)
    };
    let worker = stub_auction::Api::new(&uri);
    let wit_auction = auction.into();
    worker.initialize(&wit_auction);
}
