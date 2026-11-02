//! Binary WebSocket messages. All numbers are little-endian. The first byte is the message type.
//!
//! Client -> server
//!   SUBSCRIBE  u16 n, n x (u16 cx, u16 cy)      the full set of chunks the client wants; replaces the last one
//!   ACTION     u8 kind, u32 x, u32 y, u8 arg     kind: DIG (mine, crack a wall, collect water), WALL (build,
//!                                                 or repair a cracked wall), PLACE (arg = ITEM_...)
//!   BUY        u8 upgrade                        UPGRADE_...; answered with PLAYER
//!   BRUSH      u8 size                           dev clients only: actions cover size x size cells
//!
//! Server -> client
//!   WELCOME    u32 w, u32 h, u16 chunk, u32 spawn x, u32 spawn y, u16 tick ms,
//!              u16 cooldown ms (dig, collect, wall, place), u16 wall cost, u16 repair cost, u16 sea level,
//!              w x u16 surface
//!   CHUNKS     u16 n, n x (u16 cx, u16 cy, u16 len, len bytes run-length (value, count) pairs)
//!   DIFF       u16 n, n x (u16 cx, u16 cy, u16 count, count x (u16 cell in chunk, u8 value))
//!   PLAYER     u8 result, u32 cooldown left ms, u32 cooldown total ms, u32 rock, u32 water, u32 spongestone,
//!              u32 quartz, u8 pickaxe level, u8 flags (bit 0: walls unlocked)
//!   STATS      u16 players online, u32 record depth, u8 rain (0 dry, 1 light, 2 normal, 3 heavy),
//!              u16 action speed in percent (cooldowns are divided by it)
//!
//! Cell values: low 6 bits type, bit 6 shadow, bit 7 revealed; hidden cells are always sent as 0.

pub const C_SUBSCRIBE: u8 = 1;
pub const C_ACTION: u8 = 2;
pub const C_BUY: u8 = 3;
pub const C_BRUSH: u8 = 4;

pub const S_WELCOME: u8 = 1;
pub const S_CHUNKS: u8 = 2;
pub const S_DIFF: u8 = 3;
pub const S_PLAYER: u8 = 4;
pub const S_STATS: u8 = 5;

pub const ACT_DIG: u8 = 0;
pub const ACT_WALL: u8 = 1;
pub const ACT_PLACE: u8 = 2;

/// Items that can be placed with ACT_PLACE.
pub const ITEM_SPONGE: u8 = 0;
/// Rubble: costs RUBBLE_COST rock and the dig cooldown; dropped on water it replaces that water cell.
pub const ITEM_RUBBLE: u8 = 1;
/// A plain rock block: 1 rock and the dig cooldown; like walls it needs support in the open sky.
pub const ITEM_ROCK: u8 = 2;
/// A water cell: 1 scooped water and the collect cooldown, on an air cell only; it falls and flows.
pub const ITEM_WATER: u8 = 3;

/// Upgrades bought with C_BUY.
pub const UPGRADE_WALLS: u8 = 0;
pub const UPGRADE_PICKAXE: u8 = 1;

/// Outcome of an action, sent back in PLAYER.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ActResult {
    /// Nothing happened yet (state update without an action).
    None = 0,
    Ok = 1,
    Cooldown = 2,
    NotVisible = 3,
    WrongTarget = 4,
    NoResources = 5,
    /// The cell does not touch open air (for example the bottom of a lake).
    OutOfReach = 6,
    /// A block placed in the open sky must rest against something solid.
    Unsupported = 7,
    /// Walls have not been unlocked yet.
    Locked = 8,
}

pub enum ClientMsg {
    Subscribe(Vec<(u16, u16)>),
    Action { kind: u8, x: u32, y: u32, arg: u8 },
    Buy(u8),
    Brush(u8),
}

/// Most chunks one client may subscribe to at once.
pub const MAX_SUBSCRIBED: usize = 600;

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn u8(&mut self) -> Option<u8> {
        let (&b, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(b)
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes([self.u8()?, self.u8()?]))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes([self.u8()?, self.u8()?, self.u8()?, self.u8()?]))
    }
}

/// Parse a client message; None for anything malformed.
pub fn parse_client(data: &[u8]) -> Option<ClientMsg> {
    let mut r = Reader(data);
    match r.u8()? {
        C_SUBSCRIBE => {
            let n = r.u16()? as usize;
            if n > MAX_SUBSCRIBED {
                return None;
            }
            let chunks = (0..n).map(|_| Some((r.u16()?, r.u16()?))).collect::<Option<Vec<_>>>()?;
            Some(ClientMsg::Subscribe(chunks))
        }
        C_ACTION => Some(ClientMsg::Action { kind: r.u8()?, x: r.u32()?, y: r.u32()?, arg: r.u8()? }),
        C_BUY => Some(ClientMsg::Buy(r.u8()?)),
        C_BRUSH => Some(ClientMsg::Brush(r.u8()?)),
        _ => None,
    }
}

pub fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
