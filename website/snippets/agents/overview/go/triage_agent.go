type Email struct{ ID, Body string }

// each inbox gets its own agent, in its own WebAssembly VM:
type TriageID struct{ Inbox string }
type TriageIn struct{ Emails []Email } //@

var Triager = golem.DefineAgent[TriageID](
    golem.Spec{Name: "TriageAgent"})
var Triage = Triager.Method[TriageIn, map[string]Label]("triage")
var _ = Triager.Implement(func(id TriageID) *TriageID {
    return &id
}).Handle(Triage, golem.Adapt((*TriageID).triage))

func (t *TriageID) triage(in TriageIn) map[string]Label {
    labels := map[string]Label{}
    for _, email := range in.Emails {
        label := parseLabel(classify(email.Body))
        // it resumes here after a crash, with memory and files:
        applyLabel(t.Inbox, email.ID, label) //@
        labels[email.ID] = label
    }
    return labels
}
