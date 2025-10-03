create table chat_room_participant
(
    joined_at            timestamp(6) with time zone not null,
    last_message_read_at timestamp(6) with time zone,
    participant_state    varchar(255)                not null
        constraint chat_room_participant_participant_state_check
            check ((participant_state)::text = ANY
        ((ARRAY ['Joined'::character varying, 'Invited'::character varying, 'Left'::character varying])::text[])),
    room_id              uuid                        not null
        constraint fk677gcppc5fneuseoige64fsnm
            references chat_room,
    user_id              uuid                        not null
        constraint fkdjp8ps7q8cjcitu5e8fgkhxq0
            references app_user,
    primary key (room_id, user_id)
);

alter table chat_room_participant
    owner to postgres;

create index idx_participants_user_room_id
    on chat_room_participant (user_id, room_id);

create index idx_participants_room_id_membership
    on chat_room_participant (room_id, participant_state);


create table chat_room
(
    id                          uuid                        not null
        primary key,
    created_at                  timestamp(6) with time zone not null,
    latest_message              timestamp(6) with time zone,
    latest_message_preview_text varchar(255),
    room_image_url              varchar(255),
    room_name                   varchar(255),
    room_type                   varchar(255)                not null
        constraint chat_room_room_type_check
            check ((room_type)::text = ANY ((ARRAY ['Single'::character varying, 'Group'::character varying])::text[]))
    );

alter table chat_room
    owner to postgres;

create index idx_room_type
    on chat_room (room_type);

create index idx_room_latest_message
    on chat_room (latest_message);


create table app_user
(
    id                     uuid                        not null primary key,
    created_at             timestamp(6) with time zone not null,
    deleted_at             timestamp(6) with time zone,
    description            varchar(250),
    display_name           varchar(255)                not null,
    friends_count          bigint                      not null,
    last_modified_at       timestamp(6) with time zone,
    profile_picture        varchar(255),
    raw_name               varchar(255),
);

alter table app_user
    owner to postgres;

create index user_rawname
    on app_user (raw_name);

create unique index idx_unique_displayname_if_not_deleted
    on app_user (display_name)
    where (deleted_at IS NULL);

