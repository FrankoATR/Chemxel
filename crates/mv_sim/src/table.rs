//! Per-material properties the automaton needs, in a flat table indexed by
//! `MaterialId` (one array access per voxel, no pointer chasing).

use mv_materials::{MaterialId, MaterialRegistry, MatterState};

/// Movement class of a material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Never moves on its own (solids; energy until milestone 5).
    Static,
    Powder,
    Liquid,
    Gas,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Props {
    pub kind: Kind,
    pub density: f32,
    /// Max horizontal cells a liquid (or gas) travels in one tick (at least 1).
    pub dispersion: u8,
}

#[derive(Debug, Clone)]
pub struct MaterialTable {
    props: Vec<Props>,
}

impl MaterialTable {
    pub fn from_registry(registry: &MaterialRegistry) -> Self {
        let props = registry
            .iter()
            .map(|(_, m)| Props {
                kind: match m.state {
                    MatterState::Solid | MatterState::Energy => Kind::Static,
                    MatterState::Powder => Kind::Powder,
                    MatterState::Liquid => Kind::Liquid,
                    MatterState::Gas => Kind::Gas,
                },
                density: m.density,
                dispersion: m.dispersion.max(1),
            })
            .collect();
        Self { props }
    }

    /// Properties of `id`. Unknown ids behave as static solids.
    #[inline]
    pub fn get(&self, id: MaterialId) -> Props {
        self.props.get(id.index()).copied().unwrap_or(Props {
            kind: Kind::Static,
            density: f32::MAX,
            dispersion: 1,
        })
    }
}
