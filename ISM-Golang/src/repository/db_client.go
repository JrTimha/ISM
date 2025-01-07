package repository

import (
	"ISM/config"
	"github.com/gocql/gocql"
)

func ConnectDatabase() *gocql.Session {
	dbConfig := config.Config.DbConfig
	cluster := gocql.NewCluster(dbConfig.DbHost + ":" + dbConfig.DbPort)
	cluster.Authenticator = gocql.PasswordAuthenticator{
		Username: dbConfig.DbUser,
		Password: dbConfig.DbPassword,
	}
	cluster.Consistency = gocql.One
	if !dbConfig.DbInit {
		cluster.Keyspace = dbConfig.DbKeyspace
		session, err := cluster.CreateSession()
		if err != nil {
			panic(err)
		}
		return session
	} else {
		cluster.Keyspace = "system"
		session, err := cluster.CreateSession()
		if err != nil {
			panic(err)
		}
		err = session.Query(`
			CREATE KEYSPACE IF NOT EXISTS messaging
				WITH replication = {
					'class': 'SimpleStrategy',
					'replication_factor': 1
			}`).Exec()
		if err != nil {
			panic(err)
		}
		session.Close()
		cluster.Keyspace = dbConfig.DbKeyspace
		config.Log.Printf("Created new keyspace: %+v\n", dbConfig.DbKeyspace)
		session, err = cluster.CreateSession()
		if err != nil {
			panic(err)
		}
		err = session.Query(`
			CREATE TABLE IF NOT EXISTS messages (
				message_id UUID,
				sender_id UUID,
				receiver_id UUID,
				msg_body TEXT,
				created_at TIMESTAMP,
				msg_type TEXT,
				has_read BOOLEAN,
				PRIMARY KEY ((receiver_id), message_id, created_at)
			)`).Exec()
		return session
	}

}
