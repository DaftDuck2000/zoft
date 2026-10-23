use crate::project::Project;
use anyhow::Result;
use bincode;
use lz4_flex::compress_prepend_size;
use serde::{Deserialize, Serialize};
use slotmap::SlotMap;
use std::collections::HashMap;
use std::io::{Read, Write, Seek, SeekFrom};
use std::path::Path;

const MAGIC: [u8; 4] = *b"ZOFT";
const FILE_VERSION: u32 = 1;
const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectFileHeader {
    pub magic: [u8; 4],
    pub file_version: u32,
    pub schema_version: u32,
    pub project_id: [u8; 16],
    pub section_count: u32,
    pub section_table_offset: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionEntry {
    pub section_type: u32,
    pub offset: u64,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub checksum: u32,
}

pub mod section_types {
    pub const PROJECT_META: u32 = 0x01;
    pub const TEMPO_MAP: u32 = 0x02;
    pub const TIME_SIGNATURE_MAP: u32 = 0x03;
    pub const TRACKS: u32 = 0x04;
    pub const AUTOMATION_LANES: u32 = 0x05;
    pub const PLUGIN_INSTANCES: u32 = 0x06;
    pub const MARKER_REGIONS: u32 = 0x07;
    pub const AUDIO_CLIPS: u32 = 0x08;
    pub const MIDI_CLIPS: u32 = 0x09;
    pub const AUDIO_POOL: u32 = 0x0A;
    pub const UNDO_HISTORY: u32 = 0x0B;
}

pub struct ProjectSerialization;

impl ProjectSerialization {
    pub fn save(project: &Project, path: &Path) -> Result<()> {
        let mut file = std::fs::File::create(path)?;

        // Serialize sections
        let sections = Self::serialize_sections(project)?;

        // Write header (placeholder)
        let mut project_id = [0u8; 16];
        project_id.copy_from_slice(project.id.as_bytes());
        let header = ProjectFileHeader {
            magic: MAGIC,
            file_version: FILE_VERSION,
            schema_version: SCHEMA_VERSION,
            project_id,
            section_count: sections.len() as u32,
            section_table_offset: 0, // Will be updated
        };

        let header_bytes = bincode::serialize(&header)?;
        file.write_all(&header_bytes)?;

        let section_table_start = file.stream_position()?;

        // Write section data
        let mut section_entries = Vec::new();
        for (section_type, data) in sections {
            let compressed = compress_prepend_size(&data);
            let checksum = xxhash_rust::xxh3::xxh3_64(&compressed) as u32;

            let entry = SectionEntry {
                section_type,
                offset: file.stream_position()?,
                compressed_size: compressed.len() as u64,
                uncompressed_size: data.len() as u64,
                checksum,
            };

            file.write_all(&compressed)?;
            section_entries.push(entry);
        }

        let section_table_offset = file.stream_position()?;

        // Write section table
        for entry in &section_entries {
            let entry_bytes = bincode::serialize(entry)?;
            file.write_all(&entry_bytes)?;
        }

        // Update header with correct section table offset
        let mut header = header;
        header.section_table_offset = section_table_offset;
        let header_bytes = bincode::serialize(&header)?;

        file.seek(SeekFrom::Start(0))?;
        file.write_all(&header_bytes)?;
        file.flush()?;

        Ok(())
    }

    pub fn load(path: &Path) -> Result<Project> {
        let mut file = std::fs::File::open(path)?;

        // Header (bincode reads exactly the serialized struct; no fixed-size
        // block, which would over-read into the section data).
        let header: ProjectFileHeader = bincode::deserialize_from(&mut file)?;

        // Verify magic
        if header.magic != MAGIC {
            anyhow::bail!("Invalid file magic: not a Zoft project file");
        }

        // Verify version
        if header.file_version > FILE_VERSION {
            anyhow::bail!("File version {} is newer than supported version {}", header.file_version, FILE_VERSION);
        }

        // Read the section table sequentially; deserialize_from advances by
        // exactly the serialized size of each entry.
        file.seek(SeekFrom::Start(header.section_table_offset))?;
        let mut entries = Vec::with_capacity(header.section_count as usize);
        for _ in 0..header.section_count {
            let entry: SectionEntry = bincode::deserialize_from(&mut file)?;
            entries.push(entry);
        }

        let mut sections = HashMap::new();
        for entry in entries {
            // Read section data
            file.seek(SeekFrom::Start(entry.offset))?;
            let compressed = Self::read_exact(&mut file, entry.compressed_size as usize)?;

            // Verify checksum
            let checksum = xxhash_rust::xxh3::xxh3_64(&compressed) as u32;
            if checksum != entry.checksum {
                anyhow::bail!("Checksum mismatch for section type {}", entry.section_type);
            }

            // Decompress
            let data = lz4_flex::decompress_size_prepended(&compressed)?;
            sections.insert(entry.section_type, data);
        }

        // Deserialize project
        let mut project = Self::deserialize_sections(&sections, header.project_id)?;
        project.project_path = Some(path.to_path_buf());
        Ok(project)
    }

    fn serialize_sections(project: &Project) -> Result<Vec<(u32, Vec<u8>)>> {
        let mut sections = Vec::new();

        // Project meta
        let meta = ProjectMeta {
            name: project.name.clone(),
            sample_rate: project.sample_rate,
            bit_depth: project.bit_depth as u8,
            preferences: project.preferences.clone(),
        };
        sections.push((
            section_types::PROJECT_META,
            bincode::serialize(&meta)?,
        ));

        // Tempo map
        sections.push((
            section_types::TEMPO_MAP,
            bincode::serialize(&project.tempo_map)?,
        ));

        // Time signature map
        sections.push((
            section_types::TIME_SIGNATURE_MAP,
            bincode::serialize(&project.time_signature_map)?,
        ));

        // Tracks
        let tracks_data = TracksData {
            tracks: project.tracks.clone(),
            track_order: project.track_order.clone(),
        };
        sections.push((
            section_types::TRACKS,
            bincode::serialize(&tracks_data)?,
        ));

        // Automation lanes
        sections.push((
            section_types::AUTOMATION_LANES,
            bincode::serialize(&project.automation_lanes)?,
        ));

        // Marker regions
        sections.push((
            section_types::MARKER_REGIONS,
            bincode::serialize(&project.marker_regions)?,
        ));

        // Audio clips
        sections.push((
            section_types::AUDIO_CLIPS,
            bincode::serialize(&project.audio_clips)?,
        ));

        // MIDI clips
        sections.push((
            section_types::MIDI_CLIPS,
            bincode::serialize(&project.midi_clips)?,
        ));

        // Audio pool
        sections.push((
            section_types::AUDIO_POOL,
            bincode::serialize(&project.audio_pool)?,
        ));

        Ok(sections)
    }

    fn deserialize_sections(sections: &HashMap<u32, Vec<u8>>, project_id: [u8; 16]) -> Result<Project> {
        let mut project = Project::new("Untitled");
        project.id = uuid::Uuid::from_bytes(project_id);

        if let Some(data) = sections.get(&section_types::PROJECT_META) {
            let meta: ProjectMeta = bincode::deserialize(data)?;
            project.name = meta.name;
            project.sample_rate = meta.sample_rate;
            project.bit_depth = match meta.bit_depth {
                0 => crate::project::BitDepth::Int16,
                1 => crate::project::BitDepth::Int24,
                2 => crate::project::BitDepth::Int32,
                3 => crate::project::BitDepth::Float32,
                _ => crate::project::BitDepth::Float32,
            };
            project.preferences = meta.preferences;
        }

        if let Some(data) = sections.get(&section_types::TEMPO_MAP) {
            let tempo_map: crate::project::TempoMap = bincode::deserialize(data)?;
            project.tempo_map = tempo_map;
        }

        if let Some(data) = sections.get(&section_types::TIME_SIGNATURE_MAP) {
            let time_sig_map: crate::project::TimeSignatureMap = bincode::deserialize(data)?;
            project.time_signature_map = time_sig_map;
        }

        if let Some(data) = sections.get(&section_types::TRACKS) {
            let tracks_data: TracksData = bincode::deserialize(data)?;
            project.tracks = tracks_data.tracks;
            project.track_order = tracks_data.track_order;
        }

        if let Some(data) = sections.get(&section_types::AUTOMATION_LANES) {
            let lanes: HashMap<crate::automation::AutomationLaneId, crate::automation::AutomationLane> =
                bincode::deserialize(data)?;
            project.automation_lanes = lanes;
        }

        if let Some(data) = sections.get(&section_types::MARKER_REGIONS) {
            let markers: Vec<crate::project::MarkerRegion> = bincode::deserialize(data)?;
            project.marker_regions = markers;
        }

        if let Some(data) = sections.get(&section_types::AUDIO_CLIPS) {
            let clips: SlotMap<crate::clip::ClipId, crate::clip::AudioClip> = bincode::deserialize(data)?;
            project.audio_clips = clips;
        }

        if let Some(data) = sections.get(&section_types::MIDI_CLIPS) {
            let clips: SlotMap<crate::clip::ClipId, crate::clip::MidiClip> = bincode::deserialize(data)?;
            project.midi_clips = clips;
        }

        if let Some(data) = sections.get(&section_types::AUDIO_POOL) {
            let pool: crate::audio_pool::AudioPool = bincode::deserialize(data)?;
            project.audio_pool = pool;
        }

        Ok(project)
    }

    fn read_exact(file: &mut std::fs::File, len: usize) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; len];
        file.read_exact(&mut buf)?;
        Ok(buf)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProjectMeta {
    name: String,
    sample_rate: u32,
    bit_depth: u8,
    preferences: crate::project::Preferences,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TracksData {
    tracks: std::collections::HashMap<crate::track::TrackId, crate::track::Track>,
    track_order: Vec<crate::track::TrackId>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_pool::AudioFileRef;
    use crate::clip::{AudioClip, TimelinePos};
    use crate::project::BitDepth;
    use crate::track::Track;

    #[test]
    fn project_round_trips_through_the_zoft_file() {
        let mut project = Project::new("Round trip");
        project.add_track(Track::new_audio("Take lane"));

        // Add a clip on the first track so the audio-pool and clip sections
        // are exercised.
        let source = AudioFileRef::new(
            std::path::PathBuf::from("test.wav"),
            1_000,
            48_000,
            2,
            BitDepth::Float32,
        );
        let clip = AudioClip::new(source, TimelinePos::default(), 1_000);
        let clip_id = project.audio_clips.insert(clip);
        let first_track = project.track_order[0];
        if let Some(track) = project.get_track_mut(first_track) {
            track.add_clip(clip_id);
        }

        let path = std::env::temp_dir().join(format!(
            "zoft_round_trip_{}.zoft",
            uuid::Uuid::new_v4()
        ));
        ProjectSerialization::save(&project, &path).expect("save");
        let loaded = ProjectSerialization::load(&path).expect("load");
        std::fs::remove_file(&path).ok();

        assert_eq!(loaded.name, project.name);
        assert_eq!(loaded.sample_rate, project.sample_rate);
        assert_eq!(loaded.track_order.len(), project.track_order.len());
        assert_eq!(loaded.audio_clips.len(), 1);

        let loaded_id = loaded.audio_clips.keys().next().unwrap();
        let loaded_clip = loaded.audio_clips.get(loaded_id).unwrap();
        assert_eq!(loaded_clip.length, 1_000);
        assert_eq!(loaded_clip.source.path, std::path::PathBuf::from("test.wav"));

        let attached = loaded
            .tracks
            .values()
            .any(|track| track.clips.contains(&loaded_id));
        assert!(attached, "clip should remain attached to a track");
    }
}