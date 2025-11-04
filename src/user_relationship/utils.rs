use uuid::Uuid;
use crate::user_relationship::model::{Relationship, RelationshipState, UserRelationship};

pub fn resolve_relationship_state(
    client_id: Uuid,
    relationship: Option<UserRelationship>,
) -> Option<Relationship> {

    let relationship = relationship?;


    match relationship.state {

        RelationshipState::FRIEND => Some(Relationship::Friend),

        RelationshipState::A_BLOCKED => {
            if relationship.user_a_id == client_id {
                Some(Relationship::ClientBlocked)
            } else {
                Some(Relationship::ClientGotBlocked)
            }
        }

        RelationshipState::B_BLOCKED => {
            if relationship.user_b_id == client_id {
                Some(Relationship::ClientBlocked)
            } else {
                Some(Relationship::ClientGotBlocked)
            }
        }

        RelationshipState::ALL_BLOCKED => {
            if relationship.user_b_id == client_id || relationship.user_a_id == client_id {
                Some(Relationship::ClientBlocked)
            } else {
                Some(Relationship::ClientGotBlocked)
            }
        }

        RelationshipState::A_INVITED => {
            if relationship.user_a_id == client_id {
                Some(Relationship::InviteSent)
            } else {
                Some(Relationship::InviteReceived)
            }
        }

        RelationshipState::B_INVITED => {
            if relationship.user_b_id == client_id {
                Some(Relationship::InviteSent)
            } else {
                Some(Relationship::InviteReceived)
            }
        }
    }
}