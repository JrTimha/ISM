create table user_relationship
(
    user_a_id                     uuid                                   not null
        constraint fkph3o17werngwyisq1y6vlf25r
            references app_user,
    user_b_id                     uuid                                   not null
        constraint fkpk2xkm3f30twy5prqu8pp4wkj
            references app_user,
    state                         varchar(255)                           not null
        constraint user_relationship_state_check
            check ((state)::text = ANY
        ((ARRAY ['A_BLOCKED'::character varying, 'B_BLOCKED'::character varying, 'ALL_BLOCKED'::character varying, 'FRIEND'::character varying, 'A_INVITED'::character varying, 'B_INVITED'::character varying])::text[])),
    relationship_change_timestamp timestamp with time zone default now() not null,
    primary key (user_a_id, user_b_id),
    constraint uk_user_relationship_users
        unique (user_a_id, user_b_id),
    constraint ck_user_relationship_ids_not_equal
        check (user_a_id <> user_b_id)
);

create index idx_user_relationship_users
    on user_relationship (user_a_id, user_b_id);

create index idx_user_relationship_state
    on user_relationship (state);