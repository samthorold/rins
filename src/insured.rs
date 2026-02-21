use crate::events::Risk;
use crate::types::InsuredId;

pub struct Insured {
    pub id: InsuredId,
    pub name: String,
    pub assets: Vec<Risk>,
}
