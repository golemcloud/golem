mod auction_logic;
mod model;

mod bindings;

use bindings::*;
use exports::auction::auction_exports::api::{
    Auction as WitAuction, BidResult as WitBidResult, BidderId as WitBidderId, Guest,
};
use once_cell::sync::Lazy;
use std::cell::RefCell;

use crate::auction_logic::now;
use crate::bindings::exports::auction::auction_exports::api::GuestRunningAuction;
use model::*;

struct Component;

struct WitState {
    state: Lazy<State>,
}

static mut STATE: WitState = WitState {
    state: Lazy::new(|| State::new()),
};

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let result = unsafe { f(&mut STATE.state) };

    return result;
}

// Here, we declare a Rust implementation of the `Auction` trait.
impl Guest for Component {
    fn initialize(auction: WitAuction) {
        with_state(|state| auction_logic::intitialize(state, auction.into()))
    }

    fn bid(bidder_id: WitBidderId, price: f32) -> WitBidResult {
        with_state(|state| auction_logic::bid(state, bidder_id.into(), price).into())
    }

    fn close_auction() -> Option<WitBidderId> {
        with_state(|state| auction_logic::close_auction(state).map(|bidder_id| bidder_id.into()))
    }

    type RunningAuction = crate::RunningAuction;
}

pub struct RunningAuction {
    auction: Auction,
    winning_bid: RefCell<Option<(BidderId, f32)>>,
}

impl GuestRunningAuction for RunningAuction {
    fn new(auction: WitAuction) -> Self {
        Self {
            auction: auction.into(),
            winning_bid: RefCell::new(None),
        }
    }

    fn bid(&self, bidder_id: WitBidderId, price: f32) -> WitBidResult {
        if auction_logic::now() >= self.auction.expiration.deadline {
            return BidResult::AuctionExpired.into();
        }

        if price < self.auction.limit_price {
            return BidResult::PriceTooLow.into();
        }

        if let Some((_, winning_price)) = self.winning_bid.borrow().as_ref() {
            if price <= *winning_price {
                return BidResult::PriceTooLow.into();
            }
        }

        *self.winning_bid.borrow_mut() = Some((bidder_id.into(), price));
        BidResult::Success.into()
    }

    fn close(&self) -> Option<WitBidderId> {
        if now() >= self.auction.expiration.deadline {
            self.winning_bid
                .borrow()
                .clone()
                .map(|(bidder_id, _)| bidder_id.into())
        } else {
            None
        }
    }
}

bindings::export!(Component with_types_in bindings);
