create table if not exists chat_room_participant
(
    joined_at timestamp(6) not null,
    room_id   uuid         not null
    constraint fk677gcppc5fneuseoige64fsnm
    references chat_room,
    user_id   uuid         not null
    constraint fkdjp8ps7q8cjcitu5e8fgkhxq0
    references app_user,
    primary key (room_id, user_id)
    );

alter table chat_room_participant
    owner to postgres;

create table if not exists chat_room
(
    id         uuid                        not null primary key,
    created_at timestamp(6) with time zone not null,
    room_name  varchar(255)                not null,
    room_type  varchar(255)                not null
    constraint chat_room_room_type_check
    check ((room_type)::text = ANY ((ARRAY ['Single'::character varying, 'Group'::character varying])::text[]))
);

alter table chat_room
    owner to postgres;


create table if not exists app_user
(
    id                     uuid         not null primary key,
    created_at             timestamp(6) not null,
    description            varchar(250),
    display_name           varchar(255) not null
    constraint ukfkrbhagh65rxywp3eh8a64pt8
    unique,
    email                  varchar(255) not null
    constraint uk1j9d9a06i600gd43uu3km82jw
    unique,
    profile_picture        varchar(255),
);

alter table app_user
    owner to postgres;
