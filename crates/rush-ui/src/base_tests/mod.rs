//! Tests of rush-ui-base's modules that need the screens registered: the
//! frame's keys, places and sheets as the whole UI has them, the chores
//! the screens push, clanker's reaction, the list as drawn, and whether
//! a rush leads (hosted.rs says if it's a hosted view). They test
//! the base's code, here because only rush-ui can make a whole model
//! (new_model).

mod chores;
mod clanker;
mod frame;
mod frame_messages;
mod lead;
mod listcache;
mod timers;
