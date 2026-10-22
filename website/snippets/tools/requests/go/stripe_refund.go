func StripeRefund(
    key, charge string, amount int32,
) (*http.Response, error) {
    // Golem adds an idempotency key unless you set one:
    // a retry refunds once if Stripe honors that key:
    form := url.Values{"charge": {charge},
        "amount": {fmt.Sprint(amount)}}
    req := golem.Must(http.NewRequest("POST",
        "https://api.stripe.com/v1/refunds", //@
        strings.NewReader(form.Encode())))
    req.Header.Set("Authorization", "Bearer "+key)
    req.Header.Set("Content-Type",
        "application/x-www-form-urlencoded")
    return http.DefaultClient.Do(req) //@
}
