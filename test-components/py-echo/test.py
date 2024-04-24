from bindings.echo import exports

class Api(exports.Api):
    def echo(self, value):
       return value
