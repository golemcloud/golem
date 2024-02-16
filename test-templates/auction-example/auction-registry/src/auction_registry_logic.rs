use std::time::{Duration, SystemTime};

use crate::auction;
use crate::model::*;

pub fn create_bidder(state: &mut State, name: String, address: String) -> BidderId {
    let bidder_id = BidderId::new();
    let bidder = Bidder::new(bidder_id.clone(), name, address);
    state.bidders.insert(bidder_id.clone(), bidder);
    bidder_id
}

pub fn create_auction(
    state: &mut State,
    name: String,
    description: String,
    limit_price: f32,
    expiration: Deadline,
) -> AuctionId {
    let auction_id = AuctionId::new();
    let auction = Auction::new(
        auction_id.clone(),
        name,
        description,
        limit_price,
        expiration,
    );
    state.items.insert(auction_id.clone(), auction.clone());
    auction::create(auction.clone());
    auction_id
}

pub fn get_auctions(state: &State) -> Vec<Auction> {
    state
        .items
        .values()
        .filter(|item| item.expiration.deadline > now())
        .cloned()
        .collect()
}

fn now() -> Duration {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
}
