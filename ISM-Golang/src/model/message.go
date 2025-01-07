package model

import (
	"github.com/gocql/gocql"
	"time"
)

type Message struct {
	MessageId  gocql.UUID `json:"messageId" cql:"message_id"`
	SenderId   gocql.UUID `json:"senderId" cql:"sender_id"`
	ReceiverId gocql.UUID `json:"receiverId" cql:"receiver_id"`
	MsgBody    string     `json:"msgBody" cql:"msg_body"`
	CreatedAt  time.Time  `json:"createdAt" cql:"created_at"`
	MsgType    MsgType    `json:"msgType" cql:"msg_type"`
	HasRead    bool       `json:"hasRead" cql:"has_read"`
}

type MsgType string

const (
	Video MsgType = "Video"
	Text  MsgType = "Text"
	Link  MsgType = "Link"
)
