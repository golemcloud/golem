// Package impl is the IMPLEMENTATION of the Postgres agent. Each handler opens
// a connection through the SDK wrapper, so the executor tests exercise the
// wrapper and its durability against a real database.
package impl

import (
	"errors"
	"fmt"
	"slices"

	"agent-sdk-go/agents/pgstore"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/rdbms/postgres"
)

type state struct{}

var agent = pgstore.Agent.Implement(func(pgstore.Id) *state { return &state{} })

func open(address string) *postgres.DB {
	return golem.Must(postgres.Open(address))
}

func init() {
	agent.Handle(pgstore.Create, func(_ *golem.Context[state], in pgstore.Table) golem.Unit {
		db := open(in.Address)
		defer db.Close()
		golem.Must(db.Exec(fmt.Sprintf("CREATE TABLE %s (id BIGINT PRIMARY KEY, name TEXT NOT NULL)", in.Table)))
		return golem.Unit{}
	})
	agent.Handle(pgstore.Insert, func(_ *golem.Context[state], in pgstore.Row) uint64 {
		db := open(in.Address)
		defer db.Close()
		var affected uint64
		err := db.Transaction(func(tx *postgres.Tx) error {
			n, err := tx.Exec(fmt.Sprintf("INSERT INTO %s (id, name) VALUES ($1, $2)", in.Table), in.ID, in.Name)
			affected = n
			return err
		})
		if err != nil {
			panic(err)
		}
		return affected
	})
	agent.Handle(pgstore.InsertRolledBack, func(_ *golem.Context[state], in pgstore.Row) golem.Unit {
		db := open(in.Address)
		defer db.Close()
		tx := golem.Must(db.Begin())
		golem.Must(tx.Exec(fmt.Sprintf("INSERT INTO %s (id, name) VALUES ($1, $2)", in.Table), in.ID, in.Name))
		if err := tx.Rollback(); err != nil {
			panic(err)
		}
		return golem.Unit{}
	})
	agent.Handle(pgstore.Names, func(_ *golem.Context[state], in pgstore.Table) []string {
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
	agent.Handle(pgstore.FailingQuery, func(_ *golem.Context[state], in pgstore.Table) string {
		db := open(in.Address)
		defer db.Close()
		_, err := db.Query(fmt.Sprintf("SELECT missing_column FROM %s", in.Table))
		var pgErr *postgres.Error
		if !errors.As(err, &pgErr) {
			panic(fmt.Sprintf("expected a postgres error, got %v", err))
		}
		switch pgErr.Kind {
		case postgres.ConnectionFailure:
			return "connection-failure"
		case postgres.QueryParameterFailure:
			return "query-parameter-failure"
		case postgres.QueryExecutionFailure:
			return "query-execution-failure"
		case postgres.QueryResponseFailure:
			return "query-response-failure"
		default:
			return "other"
		}
	})
}
