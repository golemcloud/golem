type StockID struct{ SKU string }
type RestockIn struct{ N int32 }
type stock struct{ units int32 }

var Stock = golem.DefineAgent[StockID](
    golem.Spec{Name: "StockAgent"})
var Restock = Stock.Method[RestockIn, golem.Unit]("restock")
var Level = Stock.Method[golem.Unit, int32]("level",
    // repeat calls come from a cache, without loading the agent:
    // the cache clears when a restock succeeds:
    // outgoing HTTP, agent calls and key-value writes fail:
    golem.ReadOnly()) //@

var impl = Stock.Implement(func(StockID) *stock { return &stock{} })
var _ = impl.Handle(Restock, golem.AdaptUnit((*stock).restock))
var _ = impl.Handle(Level, golem.Adapt0((*stock).level))

func (s *stock) restock(in RestockIn) { s.units += in.N }
func (s *stock) level() int32         { return s.units }
