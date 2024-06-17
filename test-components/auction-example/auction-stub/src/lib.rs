#![allow(warnings)]
use golem_wasm_rpc::*;
#[allow(dead_code)]
mod bindings;
pub struct Api {
    rpc: WasmRpc,
}
impl Api {}
pub struct FutureBidResult {
    pub future_invoke_result: FutureInvokeResult,
}
pub struct FutureCloseAuctionResult {
    pub future_invoke_result: FutureInvokeResult,
}
pub struct RunningAuction {
    rpc: WasmRpc,
    id: u64,
    uri: golem_wasm_rpc::Uri,
}
impl RunningAuction {
    pub fn from_remote_handle(uri: golem_wasm_rpc::Uri, id: u64) -> Self {
        Self {
            rpc: WasmRpc::new(&uri),
            id,
            uri,
        }
    }
}
pub struct FutureRunningAuctionBidResult {
    pub future_invoke_result: FutureInvokeResult,
}
pub struct FutureRunningAuctionCloseResult {
    pub future_invoke_result: FutureInvokeResult,
}
impl crate::bindings::exports::auction::auction_stub::stub_auction::GuestFutureBidResult
for FutureBidResult {
    fn subscribe(&self) -> bindings::wasi::io::poll::Pollable {
        let pollable = self.future_invoke_result.subscribe();
        let pollable = unsafe {
            bindings::wasi::io::poll::Pollable::from_handle(pollable.into_handle())
        };
        pollable
    }
    fn get(&self) -> crate::bindings::auction::auction::api::BidResult {
        let result = self
            .future_invoke_result
            .get()
            .expect(&format!("Failed to invoke remote {}", "auction:auction/api.{bid}"));
        ({
            let (case_idx, inner) = result
                .tuple_element(0)
                .expect("tuple not found")
                .variant()
                .expect("variant not found");
            match case_idx {
                0u32 => crate::bindings::auction::auction::api::BidResult::AuctionExpired,
                1u32 => crate::bindings::auction::auction::api::BidResult::PriceTooLow,
                2u32 => crate::bindings::auction::auction::api::BidResult::Success,
                _ => unreachable!("invalid variant case index"),
            }
        })
    }
}
impl crate::bindings::exports::auction::auction_stub::stub_auction::GuestFutureCloseAuctionResult
for FutureCloseAuctionResult {
    fn subscribe(&self) -> bindings::wasi::io::poll::Pollable {
        let pollable = self.future_invoke_result.subscribe();
        let pollable = unsafe {
            bindings::wasi::io::poll::Pollable::from_handle(pollable.into_handle())
        };
        pollable
    }
    fn get(&self) -> Option<crate::bindings::auction::auction::api::BidderId> {
        let result = self
            .future_invoke_result
            .get()
            .expect(
                &format!(
                    "Failed to invoke remote {}", "auction:auction/api.{close-auction}"
                ),
            );
        (result
            .tuple_element(0)
            .expect("tuple not found")
            .option()
            .expect("option not found")
            .map(|inner| {
                let record = inner;
                crate::bindings::auction::auction::api::BidderId {
                    bidder_id: record
                        .field(0usize)
                        .expect("record field not found")
                        .string()
                        .expect("string not found")
                        .to_string(),
                }
            }))
    }
}
impl crate::bindings::exports::auction::auction_stub::stub_auction::GuestApi for Api {
    fn new(location: crate::bindings::golem::rpc::types::Uri) -> Self {
        let location = golem_wasm_rpc::Uri {
            value: location.value,
        };
        Self {
            rpc: WasmRpc::new(&location),
        }
    }
    fn blocking_initialize(
        &self,
        auction: crate::bindings::auction::auction::api::Auction,
    ) -> () {
        let result = self
            .rpc
            .invoke_and_await(
                "auction:auction/api.{initialize}",
                &[
                    WitValue::builder()
                        .record()
                        .item()
                        .record()
                        .item()
                        .string(&auction.auction_id.auction_id)
                        .finish()
                        .item()
                        .string(&auction.name)
                        .item()
                        .string(&auction.description)
                        .item()
                        .f32(auction.limit_price)
                        .item()
                        .u64(auction.expiration)
                        .finish(),
                ],
            )
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}",
                    "auction:auction/api.{initialize}"
                ),
            );
        ()
    }
    fn initialize(
        &self,
        auction: crate::bindings::auction::auction::api::Auction,
    ) -> () {
        let result = self
            .rpc
            .invoke(
                "auction:auction/api.{initialize}",
                &[
                    WitValue::builder()
                        .record()
                        .item()
                        .record()
                        .item()
                        .string(&auction.auction_id.auction_id)
                        .finish()
                        .item()
                        .string(&auction.name)
                        .item()
                        .string(&auction.description)
                        .item()
                        .f32(auction.limit_price)
                        .item()
                        .u64(auction.expiration)
                        .finish(),
                ],
            )
            .expect(
                &format!(
                    "Failed to invoke remote {}", "auction:auction/api.{initialize}"
                ),
            );
        ()
    }
    fn blocking_bid(
        &self,
        bidder_id: crate::bindings::auction::auction::api::BidderId,
        price: f32,
    ) -> crate::bindings::auction::auction::api::BidResult {
        let result = self
            .rpc
            .invoke_and_await(
                "auction:auction/api.{bid}",
                &[
                    WitValue::builder()
                        .record()
                        .item()
                        .string(&bidder_id.bidder_id)
                        .finish(),
                    WitValue::builder().f32(price),
                ],
            )
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}", "auction:auction/api.{bid}"
                ),
            );
        ({
            let (case_idx, inner) = result
                .tuple_element(0)
                .expect("tuple not found")
                .variant()
                .expect("variant not found");
            match case_idx {
                0u32 => crate::bindings::auction::auction::api::BidResult::AuctionExpired,
                1u32 => crate::bindings::auction::auction::api::BidResult::PriceTooLow,
                2u32 => crate::bindings::auction::auction::api::BidResult::Success,
                _ => unreachable!("invalid variant case index"),
            }
        })
    }
    fn bid(
        &self,
        bidder_id: crate::bindings::auction::auction::api::BidderId,
        price: f32,
    ) -> wit_bindgen::rt::Resource<FutureBidResult> {
        let result = self
            .rpc
            .async_invoke_and_await(
                "auction:auction/api.{bid}",
                &[
                    WitValue::builder()
                        .record()
                        .item()
                        .string(&bidder_id.bidder_id)
                        .finish(),
                    WitValue::builder().f32(price),
                ],
            );
        wit_bindgen::rt::Resource::new(FutureBidResult {
            future_invoke_result: result,
        })
    }
    fn blocking_close_auction(
        &self,
    ) -> Option<crate::bindings::auction::auction::api::BidderId> {
        let result = self
            .rpc
            .invoke_and_await("auction:auction/api.{close-auction}", &[])
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}",
                    "auction:auction/api.{close-auction}"
                ),
            );
        (result
            .tuple_element(0)
            .expect("tuple not found")
            .option()
            .expect("option not found")
            .map(|inner| {
                let record = inner;
                crate::bindings::auction::auction::api::BidderId {
                    bidder_id: record
                        .field(0usize)
                        .expect("record field not found")
                        .string()
                        .expect("string not found")
                        .to_string(),
                }
            }))
    }
    fn close_auction(&self) -> wit_bindgen::rt::Resource<FutureCloseAuctionResult> {
        let result = self
            .rpc
            .async_invoke_and_await("auction:auction/api.{close-auction}", &[]);
        wit_bindgen::rt::Resource::new(FutureCloseAuctionResult {
            future_invoke_result: result,
        })
    }
}
impl crate::bindings::exports::auction::auction_stub::stub_auction::GuestFutureRunningAuctionBidResult
for FutureRunningAuctionBidResult {
    fn subscribe(&self) -> bindings::wasi::io::poll::Pollable {
        let pollable = self.future_invoke_result.subscribe();
        let pollable = unsafe {
            bindings::wasi::io::poll::Pollable::from_handle(pollable.into_handle())
        };
        pollable
    }
    fn get(&self) -> crate::bindings::auction::auction::api::BidResult {
        let result = self
            .future_invoke_result
            .get()
            .expect(
                &format!(
                    "Failed to invoke remote {}",
                    "auction:auction/api.{running-auction.bid}"
                ),
            );
        ({
            let (case_idx, inner) = result
                .tuple_element(0)
                .expect("tuple not found")
                .variant()
                .expect("variant not found");
            match case_idx {
                0u32 => crate::bindings::auction::auction::api::BidResult::AuctionExpired,
                1u32 => crate::bindings::auction::auction::api::BidResult::PriceTooLow,
                2u32 => crate::bindings::auction::auction::api::BidResult::Success,
                _ => unreachable!("invalid variant case index"),
            }
        })
    }
}
impl crate::bindings::exports::auction::auction_stub::stub_auction::GuestFutureRunningAuctionCloseResult
for FutureRunningAuctionCloseResult {
    fn subscribe(&self) -> bindings::wasi::io::poll::Pollable {
        let pollable = self.future_invoke_result.subscribe();
        let pollable = unsafe {
            bindings::wasi::io::poll::Pollable::from_handle(pollable.into_handle())
        };
        pollable
    }
    fn get(&self) -> Option<crate::bindings::auction::auction::api::BidderId> {
        let result = self
            .future_invoke_result
            .get()
            .expect(
                &format!(
                    "Failed to invoke remote {}",
                    "auction:auction/api.{running-auction.close}"
                ),
            );
        (result
            .tuple_element(0)
            .expect("tuple not found")
            .option()
            .expect("option not found")
            .map(|inner| {
                let record = inner;
                crate::bindings::auction::auction::api::BidderId {
                    bidder_id: record
                        .field(0usize)
                        .expect("record field not found")
                        .string()
                        .expect("string not found")
                        .to_string(),
                }
            }))
    }
}
impl crate::bindings::exports::auction::auction_stub::stub_auction::GuestRunningAuction
for RunningAuction {
    fn new(
        location: crate::bindings::golem::rpc::types::Uri,
        auction: crate::bindings::auction::auction::api::Auction,
    ) -> Self {
        let location = golem_wasm_rpc::Uri {
            value: location.value,
        };
        let rpc = WasmRpc::new(&location);
        let result = rpc
            .invoke_and_await(
                "auction:auction/api.{running-auction.new}",
                &[
                    WitValue::builder()
                        .record()
                        .item()
                        .record()
                        .item()
                        .string(&auction.auction_id.auction_id)
                        .finish()
                        .item()
                        .string(&auction.name)
                        .item()
                        .string(&auction.description)
                        .item()
                        .f32(auction.limit_price)
                        .item()
                        .u64(auction.expiration)
                        .finish(),
                ],
            )
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}",
                    "auction:auction/api.{running-auction.new}"
                ),
            );
        ({
            let (uri, id) = result
                .tuple_element(0)
                .expect("tuple not found")
                .handle()
                .expect("handle not found");
            Self { rpc, id, uri }
        })
    }
    fn blocking_bid(
        &self,
        bidder_id: crate::bindings::auction::auction::api::BidderId,
        price: f32,
    ) -> crate::bindings::auction::auction::api::BidResult {
        let result = self
            .rpc
            .invoke_and_await(
                "auction:auction/api.{running-auction.bid}",
                &[
                    WitValue::builder().handle(self.uri.clone(), self.id),
                    WitValue::builder()
                        .record()
                        .item()
                        .string(&bidder_id.bidder_id)
                        .finish(),
                    WitValue::builder().f32(price),
                ],
            )
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}",
                    "auction:auction/api.{running-auction.bid}"
                ),
            );
        ({
            let (case_idx, inner) = result
                .tuple_element(0)
                .expect("tuple not found")
                .variant()
                .expect("variant not found");
            match case_idx {
                0u32 => crate::bindings::auction::auction::api::BidResult::AuctionExpired,
                1u32 => crate::bindings::auction::auction::api::BidResult::PriceTooLow,
                2u32 => crate::bindings::auction::auction::api::BidResult::Success,
                _ => unreachable!("invalid variant case index"),
            }
        })
    }
    fn bid(
        &self,
        bidder_id: crate::bindings::auction::auction::api::BidderId,
        price: f32,
    ) -> wit_bindgen::rt::Resource<FutureRunningAuctionBidResult> {
        let result = self
            .rpc
            .async_invoke_and_await(
                "auction:auction/api.{running-auction.bid}",
                &[
                    WitValue::builder().handle(self.uri.clone(), self.id),
                    WitValue::builder()
                        .record()
                        .item()
                        .string(&bidder_id.bidder_id)
                        .finish(),
                    WitValue::builder().f32(price),
                ],
            );
        wit_bindgen::rt::Resource::new(FutureRunningAuctionBidResult {
            future_invoke_result: result,
        })
    }
    fn blocking_close(
        &self,
    ) -> Option<crate::bindings::auction::auction::api::BidderId> {
        let result = self
            .rpc
            .invoke_and_await(
                "auction:auction/api.{running-auction.close}",
                &[WitValue::builder().handle(self.uri.clone(), self.id)],
            )
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}",
                    "auction:auction/api.{running-auction.close}"
                ),
            );
        (result
            .tuple_element(0)
            .expect("tuple not found")
            .option()
            .expect("option not found")
            .map(|inner| {
                let record = inner;
                crate::bindings::auction::auction::api::BidderId {
                    bidder_id: record
                        .field(0usize)
                        .expect("record field not found")
                        .string()
                        .expect("string not found")
                        .to_string(),
                }
            }))
    }
    fn close(&self) -> wit_bindgen::rt::Resource<FutureRunningAuctionCloseResult> {
        let result = self
            .rpc
            .async_invoke_and_await(
                "auction:auction/api.{running-auction.close}",
                &[WitValue::builder().handle(self.uri.clone(), self.id)],
            );
        wit_bindgen::rt::Resource::new(FutureRunningAuctionCloseResult {
            future_invoke_result: result,
        })
    }
}
impl Drop for RunningAuction {
    fn drop(&mut self) {
        self.rpc
            .invoke_and_await(
                "auction:auction/api.{running-auction.drop}",
                &[WitValue::builder().handle(self.uri.clone(), self.id)],
            )
            .expect("Failed to invoke remote drop");
    }
}
