type pdfType struct{}

func (pdfType) MimeTypes() []string {
    return []string{"application/pdf"}
}

type Pdf = golem.UnstructuredBinary[pdfType]
type PayslipID struct{ Employee string }
type LatestIn struct{ Caller golem.Principal }

var mount = &golem.Mount{Path: "/payslips/{employee}",
    // Golem checks the sign-in token before any agent code runs:
    Auth: true} //@
var Payslips = golem.DefineAgent[PayslipID](
    golem.Spec{Name: "PayslipAgent", HTTP: mount})
var Latest = Payslips.Method[LatestIn, *Pdf]("latest",
    golem.HTTP(golem.GET("/latest.pdf")))
var _ = Payslips.Implement(func(id PayslipID) *PayslipID {
    return &id
}).Handle(Latest, golem.Adapt((*PayslipID).latest))

func (p *PayslipID) latest(in LatestIn) *Pdf {
    // the agent decides who may open the payslip:
    c, ok := in.Caller.(golem.OidcPrincipal)
    if !ok || c.Sub != p.Employee { //@
        return nil
    }
    data := golem.Must(os.ReadFile("/payslips/latest.pdf"))
    return &Pdf{Data: data, MimeType: "application/pdf"}
}
