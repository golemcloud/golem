use golem_wasm_rpc::*;
#[allow(dead_code)]
mod bindings;
pub struct Api {
    rpc: WasmRpc,
}
impl crate::bindings::exports::auction::auction_stub::stub_auction::GuestApi for Api {
    fn new(location: crate::bindings::golem::rpc::types::Uri) -> Self {
        let location = Uri { value: location.value };
        Self {
            rpc: WasmRpc::new(&location),
        }
    }
    fn initialize(
        &self,
        auction: crate::bindings::auction::auction::api::Auction,
    ) -> () {
        let result = self
            .rpc
            .invoke_and_await(
                "auction:auction/api/initialize",
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
                &format!("Failed to invoke remote {}", "auction:auction/api/initialize"),
            );
        ()
    }
    fn bid(
        &self,
        bidder_id: crate::bindings::auction::auction::api::BidderId,
        price: f32,
    ) -> crate::bindings::auction::auction::api::BidResult {
        let result = self
            .rpc
            .invoke_and_await(
                "auction:auction/api/bid",
                &[
                    WitValue::builder()
                        .record()
                        .item()
                        .string(&bidder_id.bidder_id)
                        .finish(),
                    WitValue::builder().f32(price),
                ],
            )
            .expect(&format!("Failed to invoke remote {}", "auction:auction/api/bid"));
        ({
            let (case_idx, inner) = result.variant().expect("variant not found");
            match case_idx {
                0u32 => crate::bindings::auction::auction::api::BidResult::AuctionExpired,
                1u32 => crate::bindings::auction::auction::api::BidResult::PriceTooLow,
                2u32 => crate::bindings::auction::auction::api::BidResult::Success,
                _ => unreachable!("invalid variant case index"),
            }
        })
    }
    fn close_auction(&self) -> Option<crate::bindings::auction::auction::api::BidderId> {
        let result = self
            .rpc
            .invoke_and_await("auction:auction/api/close-auction", &[])
            .expect(
                &format!(
                    "Failed to invoke remote {}", "auction:auction/api/close-auction"
                ),
            );
        (result
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
