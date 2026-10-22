struct Pdf {}
impl @schema.AllowedMimeTypes for Pdf with fn mime_types() {
  ["application/pdf"]
}

#derive.agent
#derive.mount("/payslips/{employee}")
// Golem checks the sign-in token before any agent code runs:
#derive.mount_auth(true) //@
struct PayslipAgent {
  employee : String
}

fn PayslipAgent::new(employee : String) -> PayslipAgent {
  { employee, }
}

#derive.endpoint(get="/latest.pdf")
pub async fn PayslipAgent::latest(
  self : Self, caller : @common.Principal,
) -> @schema.UnstructuredBinary[Pdf]? {
  // the agent decides who may open the payslip:
  guard caller is Oidc({ sub, .. }) && sub == self.employee else { //@
    return None
  }
  let root = @fs.get_root_dir().unwrap()
  let pdf = @fs.read_bytes(root, "/payslips/latest.pdf").unwrap()
  let bytes = Bytes::from_fixedarray(pdf)
  Some(Inline(bytes~, mime_type=Some("application/pdf")))
}
