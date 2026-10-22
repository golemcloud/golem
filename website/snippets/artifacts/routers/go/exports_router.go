var Exports = golem.DefineHTTPRouter(golem.RouterSpec{
    Name: "ExportsRouter", Mount: "/exports"})
var csvPath = regexp.MustCompile(`^/exports/([\w-]+)\.csv$`)
var _ = Exports.HandleRaw(func(_ context.Context,
    req golem.HTTPRequest) golem.HTTPResponse {
    match := csvPath.FindStringSubmatch(req.Path)
    if req.Method != "GET" || match == nil {
        body := golem.StreamOf[[]byte]()
        return golem.HTTPResponse{Status: 404,
            Headers: []golem.HTTPHeader{}, Body: body}
    }
    id := match[1]
    // stream the CSV from the agent that builds this export:
    export := Export.Get(ExportID{ID: id})
    rows := CSV.MustCall(export, golem.Unit{}) //@
    return golem.HTTPResponse{
        Status: 200,
        Headers: []golem.HTTPHeader{
            {Name: "content-type", Value: []byte("text/csv")}, //@
            // browsers save it as a file instead of opening it:
            {Name: "content-disposition", Value: []byte( //@
                `attachment; filename="` + id + `.csv"`)}}, //@
        Body: rows,
    }
})
