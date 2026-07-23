use crate::commit::{EdgeChange, ObjectChange};
use crate::provenance::Provenance;
use crate::types::{EdgeId, ObjectId};

#[derive(Debug, Default)]
pub struct Transaction {
    pub object_changes: Vec<ObjectChange>,
    pub relationship_changes: Vec<EdgeChange>,
}

impl Transaction {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn put_object(&mut self, object_id: ObjectId, payload: Vec<u8>) {
        self.put_object_with_provenance(object_id, payload, None);
    }

    pub fn put_object_with_provenance(
        &mut self,
        object_id: ObjectId,
        payload: Vec<u8>,
        provenance: Option<Provenance>,
    ) {
        self.object_changes.push(ObjectChange {
            object_id,
            payload: Some(payload),
            provenance,
        });
    }

    pub fn delete_object(&mut self, object_id: ObjectId) {
        self.object_changes.push(ObjectChange {
            object_id,
            payload: None,
            provenance: None,
        });
    }

    pub fn put_edge(
        &mut self,
        edge_id: EdgeId,
        from_object: ObjectId,
        to_object: ObjectId,
        relationship_type: impl Into<String>,
        payload: Option<Vec<u8>>,
    ) {
        self.put_edge_with_provenance(
            edge_id,
            from_object,
            to_object,
            relationship_type,
            payload,
            None,
        );
    }

    pub fn put_edge_with_provenance(
        &mut self,
        edge_id: EdgeId,
        from_object: ObjectId,
        to_object: ObjectId,
        relationship_type: impl Into<String>,
        payload: Option<Vec<u8>>,
        provenance: Option<Provenance>,
    ) {
        self.relationship_changes.push(EdgeChange {
            edge_id,
            from_object: Some(from_object),
            to_object: Some(to_object),
            relationship_type: Some(relationship_type.into()),
            payload,
            provenance,
        });
    }

    pub fn delete_edge(&mut self, edge_id: EdgeId) {
        self.relationship_changes.push(EdgeChange {
            edge_id,
            from_object: None,
            to_object: None,
            relationship_type: None,
            payload: None,
            provenance: None,
        });
    }

    pub fn is_empty(&self) -> bool {
        self.object_changes.is_empty() && self.relationship_changes.is_empty()
    }
}
