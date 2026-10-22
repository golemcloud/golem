// asked to export results as CSV, the agent replies with a link:
// the link is /exports/<chat>/<random id>.csv, for that user only:
var Exports = golem.DefineHTTPRouter(golem.RouterSpec{
    Name: "ExportsRouter", Mount: "/exports"})
var csvPath = regexp.MustCompile(
    `^/exports/([\w-]+)/([\w-]{36})\.csv$`)
var _ = Exports.HandleRaw(func(_ context.Context,
    req golem.HTTPRequest) golem.HTTPResponse {
    m := csvPath.FindStringSubmatch(req.Path) //@
    if req.Method != "GET" || m == nil { //@
        return golem.HTTPResponse{Status: 404, Headers: nil, //@
            Body: golem.StreamOf[[]byte]()} //@
    }
    // that user's agent builds the CSV now and streams it:
    csv := ExportCsv.MustCall(Analyst.Get(AnalystID{Chat: m[1]}), //@
        ExportIn{ID: m[2]})
    return golem.HTTPResponse{
        Status: 200,
        Headers: []golem.HTTPHeader{
            {Name: "content-type", Value: []byte("text/csv")}, //@
            // the browser saves it as a download:
            {Name: "content-disposition", Value: []byte( //@
                `attachment; filename="results.csv"`)}}, //@
        Body: csv, //@
    }
})
