import { CrmFindCustomerClient }
  from 'crm-find-customer-tool-guest-client'

export const Account = defineAgent({
  name: 'AccountAgent',
  id: { team: z.string() },
  methods: {
    describe: method({
      // over MCP, this method becomes a tool with this description:
      description: 'Summarize a customer account by email', //@
      input: { email: z.string() }, returns: z.string(),
    }),
  },
}).implement({
  init: () => ({}),
  methods: {
    async describe({ email }) {
      // call the CRM's MCP tool through its generated typed client:
      const call = CrmFindCustomerClient.newClient() //@
        .crm_find_customer(email) //@
      const { result } = await call.collect()
      if (result.status === 'rejected') throw result.reason
      // the MCP server's output schema types this result:
      const { name, plan } = result.value.structured //@
      return `${name} is on the ${plan} plan`
    },
  },
})
