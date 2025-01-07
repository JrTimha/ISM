package api

import (
	"ISM/config"
	"ISM/model"
	"ISM/repository"
	"fmt"
	"github.com/gin-gonic/gin"
	"github.com/gocql/gocql"
	"github.com/google/uuid"
	"time"
)

func Initialize() {
	ismConfig := config.Config
	config.Log.Printf("Initialized config: %+v", ismConfig)

	// Creates a gin router with default middleware:
	router := gin.Default()
	session := repository.ConnectDatabase()

	msgRepository := repository.NewMessageRepository(session)

	router.GET("/ping", func(c *gin.Context) {
		save, ms := msgRepository.Save(&model.Message{
			MessageId:  gocql.TimeUUID(),
			SenderId:   gocql.MustRandomUUID(),
			ReceiverId: gocql.MustRandomUUID(),
			MsgBody:    "Hello world",
			CreatedAt:  time.Now(),
			MsgType:    "Text",
			HasRead:    false,
		})
		if ms != nil {
			fmt.Println("insert failed")
			fmt.Printf("%+v\n", ms)
		}
		fmt.Printf("%+v\n", save)
		c.JSON(200, gin.H{
			"message": "ping",
		})
	})

	router.GET("/pong", func(c *gin.Context) {
		msgId, _ := uuid.Parse("16b2bf34-cd15-11ef-b568-00d861764e97")
		recId, _ := uuid.Parse("0bcdc601-f2e0-41d8-ae2c-def8436689d2")
		msg, err := msgRepository.GetById(gocql.UUID(msgId), gocql.UUID(recId))
		if err != nil {
			fmt.Println(err)
		}
		fmt.Printf("%+v\n", msg)

		c.JSON(200, gin.H{
			"message": "pong",
		})
	})

	err := router.Run(ismConfig.Host + ":" + ismConfig.Port)
	if err != nil {
		panic("Error starting server: " + err.Error())
	}
}
