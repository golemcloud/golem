// Package mysqlstore is the DEFINITION of the agent exercising the SDK's MySQL
// wrapper (golem/rdbms/mysql). Behaviour lives in mysqlstore/impl.
package mysqlstore

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Name string }

type Table struct {
	Address string
	Table   string
}

type Row struct {
	Address string
	Table   string
	ID      int64
	Name    string
}

var Agent = golem.DefineAgent[Id](golem.Spec{
	Name: "MySQLAgent", Description: "Exercises the Go SDK MySQL wrapper", Mode: golem.Durable,
})

var (
	Create = Agent.Method[Table, golem.Unit]("create", golem.Description("Create the table"))
	// Insert inserts a row in a committed transaction and returns the affected row count.
	Insert = Agent.Method[Row, uint64]("insert", golem.Description("Insert a row and commit"))
	// InsertRolledBack inserts a row in a transaction it rolls back.
	InsertRolledBack = Agent.Method[Row, golem.Unit]("insert_rolled_back", golem.Description("Insert a row and roll back"))
	// Names reads the names ordered by id, once with Query and once with QueryStream,
	// and fails when the two disagree.
	Names = Agent.Method[Table, []string]("names", golem.Description("Read the names ordered by id"))
	// FailingQuery runs invalid SQL and returns the error kind.
	FailingQuery = Agent.Method[Table, string]("failing_query", golem.Description("Run invalid SQL and report the error kind"))
)
