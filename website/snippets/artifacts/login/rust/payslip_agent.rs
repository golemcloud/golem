#[derive(AllowedMimeTypes, Clone, Debug)]
pub enum PdfType { #[mime_type("application/pdf")] Pdf }
type Pdf = UnstructuredBinary<PdfType>;

#[agent_definition(mount = "/payslips/{employee}",
  // Golem checks the sign-in token before any agent code runs:
  auth = true)] //@
pub trait PayslipAgent {
  fn new(employee: String) -> Self;
  #[endpoint(get = "/latest.pdf")]
  fn latest(&self, caller: Principal)
    -> Result<Option<Pdf>, AppError>;
}
struct PayslipAgentImpl { employee: String }
#[agent_implementation]
impl PayslipAgent for PayslipAgentImpl {
  fn new(employee: String) -> Self { Self { employee } }
  fn latest(&self, caller: Principal)
    -> Result<Option<Pdf>, AppError> {
    // the agent decides who may open the payslip:
    let allowed = matches!(&caller, //@
      Principal::Oidc(user) if user.sub == self.employee); //@
    if !allowed { return Ok(None); }
    let data = fs::read("/payslips/latest.pdf")?;
    Ok(Some(Pdf::Inline { data, mime_type: PdfType::Pdf }))
  }
}
