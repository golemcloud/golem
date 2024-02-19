use std::time::{Duration, SystemTime};

use crate::model::*;

pub fn intitialize(state: &mut State, auction: Auction) {
    state.auction = Some(auction);
}

pub fn bid(state: &mut State, bidder_id: BidderId, price: f32) -> BidResult {
    let auction = state.auction.clone().unwrap();
    let expiration = auction.expiration;
    let winning_bid = state.winning_bid.clone();

    if now() >= expiration.deadline {
        return BidResult::AuctionExpired;
    }

    if price < auction.limit_price {
        return BidResult::PriceTooLow;
    }

    if let Some((_, winning_price)) = winning_bid {
        if price <= winning_price {
            return BidResult::PriceTooLow;
        }
    }

    state.winning_bid = Some((bidder_id, price));
    BidResult::Success
}

pub fn close_auction(state: &mut State) -> Option<BidderId> {
    let auction = state.auction.clone().unwrap();
    let expiration = auction.expiration;
    let winning_bid = state.winning_bid.clone();

    if now() >= expiration.deadline {
        return winning_bid.map(|(bidder_id, _)| bidder_id);
    }

    None
}

fn now() -> Duration {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
}
