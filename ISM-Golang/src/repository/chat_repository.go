package repository

import (
	"ISM/model"
	"github.com/gocql/gocql"
)

type MessageRepository interface {
	Save(msg *model.Message) (*model.Message, error)
	GetById(msgId gocql.UUID, recId gocql.UUID) (*model.Message, error)
}

type messageRepository struct {
	session *gocql.Session
}

func NewMessageRepository(session *gocql.Session) MessageRepository {
	return &messageRepository{session: session}
}

func (repository *messageRepository) GetById(msgId gocql.UUID, recId gocql.UUID) (*model.Message, error) {
	msg := &model.Message{}
	err := repository.session.Query(`SELECT message_id, sender_id, receiver_id, msg_body, created_at, msg_type, has_read FROM messages WHERE receiver_id = ? AND message_id = ? LIMIT 1`, recId, msgId).Scan(
		&msg.MessageId, &msg.SenderId, &msg.ReceiverId, &msg.MsgBody, &msg.CreatedAt, &msg.MsgType, &msg.HasRead)
	return msg, err
}

func (repository *messageRepository) Save(msg *model.Message) (*model.Message, error) {
	err := repository.session.Query(
		`INSERT INTO messages (message_id, sender_id, receiver_id, msg_body, created_at, msg_type, has_read) VALUES (?, ?, ?, ?, ?, ?, ?)`,
		msg.MessageId, msg.SenderId, msg.ReceiverId, msg.MsgBody, msg.CreatedAt, msg.MsgType, msg.HasRead,
	).Exec()
	return msg, err
}
