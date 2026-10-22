// this router serves exports from durable agents:
var Exports = golem.DefineHTTPRouter(golem.RouterSpec{
    Name: "ExportsRouter", Mount: "/exports"})
var csvPath = regexp.MustCompile(`^/exports/([\w-]+)\.csv$`)
var _ = Exports.HandleRaw(func(_ context.Context,
    req golem.HTTPRequest) golem.HTTPResponse {
    match := csvPath.FindStringSubmatch(req.Path) //@
    if req.Method != "GET" || match == nil { //@
        return golem.HTTPResponse{Status: 404, //@
            Body: golem.StreamOf[[]byte]()}
    }
    // the durable agent behind each export streams its CSV:
    export := Export.Get(ExportID{ID: match[1]})
    rows := CSV.MustCall(export, golem.Unit{}) //@
    return golem.HTTPResponse{
        Status: 200,
        Headers: []golem.HTTPHeader{
            {Name: "content-type", Value: []byte("text/csv")},
            {Name: "content-disposition", //@
                Value: []byte("attachment")}},
        Body: rows, //@
    }
})
