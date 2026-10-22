#derive.tool("refund", version="1.0.0")
struct Refund {}

enum RefundError {
  #derive.error(kind="runtime", exit_code="1")
  Http
}

// each call gets a fresh VM that can't read the agent's memory:
pub async fn Refund::refund( //@
  charge : String,
  amount : Int,
  // api_key is an opaque handle; revealing it takes a grant:
  api_key : @secrets.SecretHandle[String],
) -> Result[RefundResult, RefundError] {
  let key = api_key.reveal()
  // on replay, a completed request returns its recorded response:
  let res = stripe_refund(key, charge, amount) //@
  guard res.get_status_code() / 100 == 2 else { return Err(Http) }
  Ok(@json.from_json(@json.parse(read_text(res))))
}
