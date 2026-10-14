// Package impl is the IMPLEMENTATION of the MySQL agent. Each handler opens
// a connection through the SDK wrapper, so the executor tests exercise the
// wrapper and its durability against a real database.
package impl

import (
	"errors"
	"fmt"
	"slices"

	"agent-sdk-go/agents/mysqlstore"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/rdbms/mysql"
)

type state struct{}

var agent = mysqlstore.Agent.Implement(func(mysqlstore.Id) *state { return &state{} })

func open(address string) *mysql.DB {
	return golem.Must(mysql.Open(address))
}

func init() {
	agent.Handle(mysqlstore.Create, func(_ *golem.Context[state], in mysqlstore.Table) golem.Unit {
		db := open(in.Address)
		defer db.Close()
		golem.Must(db.Exec(fmt.Sprintf("CREATE TABLE %s (id BIGINT PRIMARY KEY, name TEXT NOT NULL)", in.Table)))
		return golem.Unit{}
	})
	agent.Handle(mysqlstore.Insert, func(_ *golem.Context[state], in mysqlstore.Row) uint64 {
		db := open(in.Address)
		defer db.Close()
		var affected uint64
		err := db.Transaction(func(tx *mysql.Tx) error {
			n, err := tx.Exec(fmt.Sprintf("INSERT INTO %s (id, name) VALUES (?, ?)", in.Table), in.ID, in.Name)
			affected = n
			return err
		})
		if err != nil {
			panic(err)
		}
		return affected
	})
	agent.Handle(mysqlstore.InsertRolledBack, func(_ *golem.Context[state], in mysqlstore.Row) golem.Unit {
		db := open(in.Address)
		defer db.Close()
		tx := golem.Must(db.Begin())
		golem.Must(tx.Exec(fmt.Sprintf("INSERT INTO %s (id, name) VALUES (?, ?)", in.Table), in.ID, in.Name))
		if err := tx.Rollback(); err != nil {
			panic(err)
		}
		return golem.Unit{}
	})
	agent.Handle(mysqlstore.Names, func(_ *golem.Context[state], in mysqlstore.Table) []string {
		db := open(in.Address)
		defer db.Close()
		sql := fmt.Sprintf("SELECT name FROM %s ORDER BY id", in.Table)
		var queried []string
		for _, row := range golem.Must(db.Query(sql)).Rows() {
			queried = append(queried, golem.Must(row.String(0)))
		}
		stream := golem.Must(db.QueryStream(sql))
		defer stream.Close()
		var streamed []string
		for {
			row, ok, err := stream.Next()
			if err != nil {
				panic(err)
			}
			if !ok {
				break
			}
			streamed = append(streamed, golem.Must(row.String(0)))
		}
		if !slices.Equal(queried, streamed) {
			panic(fmt.Sprintf("query returned %v, stream returned %v", queried, streamed))
		}
		return queried
	})
	agent.Handle(mysqlstore.FailingQuery, func(_ *golem.Context[state], in mysqlstore.Table) string {
		db := open(in.Address)
		defer db.Close()
		_, err := db.Query(fmt.Sprintf("SELECT missing_column FROM %s", in.Table))
		var dbErr *mysql.Error
		if !errors.As(err, &dbErr) {
			panic(fmt.Sprintf("expected a mysql error, got %v", err))
		}
		switch dbErr.Kind {
		case mysql.ConnectionFailure:
			return "connection-failure"
		case mysql.QueryParameterFailure:
			return "query-parameter-failure"
		case mysql.QueryExecutionFailure:
			return "query-execution-failure"
		case mysql.QueryResponseFailure:
			return "query-response-failure"
		default:
			return "other"
		}
	})
}
