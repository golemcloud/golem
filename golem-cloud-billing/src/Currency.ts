/**
 * The currencies of the paid plan, as Polar names them. The Polar product has its prices in each
 * one. The checkout selects the currency from the location of the customer, and USD for every
 * country without a price of its own. A subscription keeps the currency of its checkout, and its
 * orders and their settlements use it too.
 *
 * An amount of money is in the minor unit of its currency: cents for USD and EUR, pence for GBP.
 */
export const currencies = ["usd", "eur", "gbp"] as const

export type Currency = (typeof currencies)[number]

/**
 * A value for each currency, for example the overage prices of the paid plan. A setting whose
 * amount depends on the currency has this shape.
 */
export type PerCurrency<A> = { readonly [C in Currency]: A }

export const isCurrency = (value: string): value is Currency => (currencies as ReadonlyArray<string>).includes(value)

/**
 * The default currency of the Polar organization. A checkout uses it for a customer in a country
 * without a price of its own, and Polar refuses a list of prices without a price in it.
 */
export const organizationCurrency: Currency = "usd"

/** The value of `make` for each currency. */
export const perCurrency = <A>(make: (currency: Currency) => A): PerCurrency<A> => ({
  usd: make("usd"),
  eur: make("eur"),
  gbp: make("gbp")
})

/** The ISO 4217 code of the currency: `USD`, `EUR` or `GBP`. */
export const isoCode = (currency: Currency): string => currency.toUpperCase()

/**
 * Whether the prices of the Polar product in the currency include the tax. Polar adds the tax on
 * top of a USD price (`tax_behavior` `exclusive`), and takes it out of a EUR or GBP price
 * (`inclusive`), so a customer in the EU or the UK sees what they pay. The prices of the product
 * must have this tax behavior.
 */
export const taxIncluded: PerCurrency<boolean> = { usd: false, eur: true, gbp: true }
