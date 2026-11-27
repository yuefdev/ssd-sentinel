export type Metric = number | string | null;
export interface SmartRow {
  id: string;
  name: string;
  original: string;
  raw: string;
  value: number | null;
  worst: number | null;
  threshold: number | null;
  description: string;
  level: string;
}
export interface Volume {
  letter: string;
  label: string | null;
  filesystem: string;
  size_bytes: number | null;
  free_bytes: number | null;
}
export interface BusLink {
  kind?: string;
  current?: string;
  max?: string;
  current_gen?: number;
  max_gen?: number;
  current_width?: number;
  max_width?: number;
  ceiling_mb_s?: number | null;
  bottleneck?: boolean;
}
export interface DiskOptimization {
  write_cache?: {
    type?: string;
    enabled?: boolean | null;
    changeable?: boolean;
    flush_supported?: boolean;
  } | null;
  aligned_4k?: boolean | null;
  misaligned_partitions?: number | null;
  allocated_bytes?: number | null;
  unallocated_bytes?: number | null;
  over_provisioning_percent?: number | null;
}
export interface Disk {
  id: string;
  stable_identity: boolean;
  device: string;
  device_type: string;
  model: string;
  serial: string | null;
  firmware: string | null;
  type: string;
  protocol: string | null;
  metrics: Record<string, Metric>;
  health: {
    percent: number | null;
    status: string;
    method: string | null;
    explanation: string;
  };
  smart: SmartRow[];
  alerts: { code: string; level: string; title: string; detail: string }[];
  capabilities: {
    self_test: boolean;
    trim: boolean | null;
    logs: boolean | null;
  };
  interface: string | null;
  volumes: Volume[];
  os_disk_number: number | null;
  os_serial: string | null;
  os_model: string | null;
  bus_link?: BusLink | null;
  optimization?: DiskOptimization | null;
  read_incomplete?: boolean;
  read_warning?: string | null;
}
export interface ProfileRow {
  id: string;
  name: string;
  block_kib: number;
  queue: number;
  threads: number;
  random: boolean;
}
export interface Measurement {
  mb_s: number;
  iops: number;
  latency_us: number | null;
  seconds: number;
  read_bytes: string;
  write_bytes: string;
}
export interface Block {
  index: number;
  status: string;
  latency_ms: number | null;
  mb_s: number | null;
  offset_bytes: number;
  bytes: number;
  file: string;
  error: string | null;
}
export interface SmartDeltaSnapshot {
  health_percent?: number | null;
  temperature_c?: number | null;
  reallocated_5: number;
  pending_197: number;
  uncorrectable_198: number;
}
export interface ZbrProfile {
  outer_mb_s?: number | null;
  inner_mb_s?: number | null;
  drop_percent?: number | null;
  tail_bad_count?: number;
  tail_degraded?: boolean;
  suggested_tail_guard_pct?: number | null;
}
export interface SurfaceTarget {
  offset: number;
  len: number;
  kind: "slow_area" | "weak_sector" | "unreadable";
  latency_ms?: number | null;
}
export interface SurfaceDiagnosis {
  score: number;
  verdict: "healthy" | "attention" | "repairable" | "failing" | string;
  title: string;
  advice: string;
  recommended_mode?: "magnetic_refresh" | "bad_zone_isolate" | null;
  unreadable: number;
  weak: number;
  slow: number;
  dead_zones: number;
  engine?: string;
}
export interface PinpointSpeed {
  before_avg_ms?: number | null;
  after_avg_ms?: number | null;
  measured: number;
}
export interface PinpointRepairRow {
  lba: number;
  offset_bytes: number;
  bytes: number;
  kind: string;
  kind_label: string;
  dead_zone?: boolean;
  action: string;
  result: string;
  status: "ok" | "warning" | "bad" | string;
  scan_ms?: number | null;
  before_ms?: number | null;
  after_ms?: number | null;
  moved_files?: string[];
  moved_count?: number;
  lost_files?: string[];
  system_files?: string[];
  blocked?: string[];
  dead_errors?: string[];
}
export interface RepairSummary {
  mode?: string;
  scope?: string;
  output?: string;
  disk_bytes?: number;
  micro_probed_sectors?: number;
  recovered_pending?: number;
  evacuated_clusters?: number;
  isolated_clusters?: number;
  isolated_bytes?: number;
  thermal_pauses?: number;
  refreshed_blocks?: number;
  skipped_locked?: number;
  full_surface?: boolean;
  engine?: string;
  targets?: SurfaceTarget[];
  diagnosis?: SurfaceDiagnosis | null;
  slow_threshold_ms?: number;
  median_chunk_ms?: number | null;
  scan_finished_at?: string;
  targets_total?: number;
  regions?: number;
  outside_volume?: number;
  healed?: number;
  quarantined_runs?: number;
  unplaceable?: number;
  relocated_files?: number;
  relocated_bytes?: number;
  data_loss_files?: number;
  speed?: PinpointSpeed | null;
  repairs?: PinpointRepairRow[];
  advice?: string[];
  scanned_bytes?: number;
  elapsed_seconds?: number;
  avg_mb_s?: number;
  slow_chunks?: number;
  transient_slow?: number;
  error_chunks?: number;
  unreadable_bytes?: number;
  bad_lbas?: {
    lba: number;
    offset_bytes: number;
    latency_ms?: number | null;
    error?: string | null;
  }[];
  zbr_profile?: ZbrProfile | null;
  smart_before?: SmartDeltaSnapshot | null;
  smart_after?: SmartDeltaSnapshot | null;
  filesystem_check?: { ok?: boolean; code?: number; output?: string } | null;
  files_consolidated?: number;
  files_placed_outer?: number;
  extents_before?: number;
  extents_after?: number;
  bytes_written?: number;
  bytes_moved?: number;
  retrim_ok?: boolean;
  [key: string]: unknown;
}
export interface TierCompanion {
  disk_id: string;
  model: string;
  letter: string;
  type: string;
  free_bytes: number;
  role?: "hdd_archive" | "ssd_boost" | string | null;
}
export interface TierCandidate {
  id: string;
  name: string;
  path: string;
  is_dir: boolean;
  size_bytes: number;
  file_count: number;
  days_untouched: number;
  category:
    | "video"
    | "iso"
    | "archive"
    | "installer"
    | "photo"
    | "audio"
    | "game"
    | "code"
    | "database"
    | "folder"
    | "other"
    | string;
  score: number;
  reason_tr: string;
  recommended_direction: "to_hdd" | "to_ssd" | string;
}
export interface TierMoveRecord {
  id: string;
  source_path: string;
  target_path: string;
  source_letter: string;
  target_letter: string;
  is_dir: boolean;
  junction_created: boolean;
  size_bytes: number;
  file_count: number;
  direction: "to_hdd" | "to_ssd" | string;
  moved_at: string;
  cleanup_warning?: string | null;
}
export interface TierAnalysis {
  disk_id: string;
  letter: string;
  disk_type: string;
  is_ssd: boolean;
  replay?: boolean;
  fill_percent: number;
  free_bytes: number;
  size_bytes: number;
  slc_pressure: "low" | "moderate" | "high" | string;
  slc_note_tr: string;
  extreme_fragmented_count: number;
  worst_extents: number;
  companion_role?: "hdd_archive" | "ssd_boost" | string | null;
  companion: TierCompanion | null;
  total_reclaimable_bytes: number;
  candidates: TierCandidate[];
  active_moves: TierMoveRecord[];
  summary_tr: string;
}
export interface SmartTierSummary {
  mode: "ssd_extent_optimize" | "hdd_outer_optimize" | string;
  files_consolidated?: number;
  files_placed_outer?: number;
  extents_before?: number;
  extents_after?: number;
  bytes_written?: number;
  bytes_moved?: number;
  retrim_ok?: boolean;
  scope?: string;
}
export interface RecoveredItem {
  id: string;
  name: string;
  original_path: string;
  extension: string;
  category: string;
  sub_type?: string;
  product_name?: string | null;
  publisher?: string | null;
  size_bytes: number;
  deleted_at?: string | null;
  source: string;
  confidence: number;
  status_label: string;
  entropy: number;
  trim_zeroed: boolean;
  ai_summary: string;
  source_letter: string;
  media_status?: "intact" | "repaired" | "thumbnail_rescued" | "unrecoverable" | string | null;
  openable?: boolean;
  repair_note?: string | null;
}
export interface DeletedAppBundle {
  name: string;
  publisher?: string | null;
  exe_path: string;
  installer_found: boolean;
  recoverable_item_id?: string | null;
  appdata_paths: string[];
  winget_hint: string;
  confidence: number;
  sources?: string[];
  run_count?: number | null;
  last_run_at?: string | null;
  vss_recoverable?: boolean;
  category?: "Oyun" | "Uygulama" | "Araç" | string;
  recovery_status?: "full_restore" | "installer_found" | "reinstall_only" | string;
  leftover_bytes?: number;
  install_dir?: string | null;
  drive_letter?: string;
}
export type RecoveryPreview =
  | {
      kind: "image";
      mime: string;
      data_url: string;
      size_bytes: number;
      repaired?: boolean;
      repair_note?: string | null;
      media_status?: string | null;
      width?: number | null;
      height?: number | null;
    }
  | {
      kind: "text";
      text: string;
      raw_text?: string;
      secrets_masked?: number;
      truncated: boolean;
      size_bytes: number;
      repaired?: boolean;
      repair_note?: string | null;
    }
  | {
      kind: "binary";
      metadata: {
        product_name?: string | null;
        publisher?: string | null;
        sub_type?: string | null;
        size_bytes?: number;
        source?: string;
        status_label?: string;
        [key: string]: unknown;
      };
      repaired?: boolean;
      repair_note?: string | null;
    }
  | {
      kind: "unavailable";
      reason: string;
      repaired?: boolean;
      repair_note?: string | null;
      media_status?: string | null;
    };
export interface BatchRestoreResult {
  saved?: boolean;
  cancelled?: boolean;
  restored: number;
  failed: number;
  skipped: number;
  repaired?: boolean;
  repaired_count?: number;
  repair_note?: string | null;
  dest_dir: string;
  paths: string[];
  errors: string[];
}
export interface HealthForecast {
  ready: boolean;
  sample_count?: number;
  span_hours?: number;
  daily_written_gb?: number | null;
  estimated_remaining_years?: number | null;
  pending_delta_7d?: number | null;
  reallocated_delta_7d?: number | null;
  uncorrectable_delta_7d?: number | null;
  temp_avg_7d?: number | null;
  risk_level?: "ok" | "warning" | "critical" | string;
  summary_tr?: string;
  advice_tr?: string;
  [key: string]: unknown;
}
export interface SurfaceCheckpoint {
  disk_id?: string;
  letter?: string;
  next_zone?: number;
  total_zones?: number;
  completed_zones?: number;
  progress_percent?: number;
  done_bytes?: number;
  disk_bytes?: number;
  elapsed_seconds?: number;
  slow_chunks?: number;
  error_chunks?: number;
  targets_count?: number;
  updated_at?: string | null;
  [key: string]: unknown;
}
export interface Job {
  id: string;
  kind: string;
  disk_id: string;
  model: string;
  letter: string;
  options: Record<string, string | number | boolean>;
  status: string;
  phase: string;
  progress: number;
  completed: number;
  total: number;
  started_at: string;
  finished_at: string | null;
  error: string | null;
  cleanup_error: string | null;
  read_bytes: number;
  write_bytes: number;
  prepared_bytes: number;
  results: {
    row: ProfileRow;
    direction: string;
    best: Measurement;
    samples: Measurement[];
  }[];
  blocks: Block[];
  summary: Record<string, number | null>;
  damaged_files: { path: string; block: number; error: string }[];
  repair_summary?: RepairSummary;
  recovery_items?: RecoveredItem[];
  deleted_apps?: DeletedAppBundle[];
}
export interface Event {
  id: number;
  disk_id: string;
  time: string;
  code: string;
  level: string;
  message: string;
}
export interface History {
  time: string;
  health_percent: number | null;
  temperature_c: number | null;
  [key: string]: unknown;
}
export interface Performance {
  number: number;
  name: string;
  read_bps: number;
  write_bps: number;
  read_iops: number;
  write_iops: number;
  read_latency_ms: number | null;
  write_latency_ms: number | null;
  busy_percent: number;
  queue_length: number;
}
export interface SystemTuning {
  trim_enabled?: boolean | null;
  hibernate_enabled?: boolean | null;
  hiberfil_bytes?: number | null;
  sysmain_running?: boolean | null;
  wsearch_running?: boolean | null;
  pcie_aspm?: number | null;
  autostart_enabled?: boolean;
  long_paths_enabled?: boolean | null;
  ntfs_8dot3_creation?: number | null;
  dev_drive_supported?: boolean | null;
}
export interface DevArtifact {
  id: string;
  name: string;
  path: string;
  size_bytes: number;
  file_count: number;
  clean_token: string;
}
export interface DevProject {
  id: string;
  name: string;
  path: string;
  stack: string;
  days_untouched: number;
  stale: boolean;
  has_git: boolean;
  has_env: boolean;
  recent_git_edit: boolean;
  artifact_bytes: number;
  artifacts: DevArtifact[];
}
export interface DevCacheItem {
  id: string;
  name: string;
  name_en: string;
  kind: "ai_model" | "pkg_cache" | "toolchain" | string;
  path: string;
  size_bytes: number;
  file_count: number;
  days_untouched: number;
  is_junction: boolean;
  can_offload: boolean;
  can_clean: boolean;
  clean_token?: string | null;
  note_tr: string;
  note_en: string;
}
export interface WslVhdxItem {
  id: string;
  name: string;
  kind: "docker_vhdx" | "wsl_vhdx" | string;
  path: string;
  drive_letter: string;
  size_bytes: number;
  modified_at?: string | null;
}
export interface DevOffloadTarget {
  disk_id: string;
  model: string;
  letter: string;
  type: string;
  free_bytes: number;
  direction: "to_hdd" | "to_archive" | string;
}
export interface DevWorkspaceAnalysis {
  disk_id: string;
  letter: string;
  replay?: boolean;
  scanned_roots: string[];
  scan_ms: number;
  truncated: boolean;
  total_artifact_bytes: number;
  stale_artifact_bytes: number;
  total_cache_bytes: number;
  total_vhdx_bytes: number;
  total_reclaimable_bytes: number;
  offload_target: DevOffloadTarget | null;
  projects: DevProject[];
  global_caches: DevCacheItem[];
  vhdx_files: WslVhdxItem[];
  summary_tr: string;
  summary_en: string;
}
export interface OptimizationRecord {
  key: string;
  target: string;
  title: string;
  previous_value: string;
  applied_value: string;
  updated_at: string;
}
export interface AppState {
  source: string;
  admin: boolean;
  updating: boolean;
  stale: boolean;
  busy: boolean;
  updated_at: string | null;
  error: string | null;
  partial_warnings?: string[];
  disks: Disk[];
  windows: {
    trim_enabled?: boolean | null;
    system_tuning?: SystemTuning;
    error?: string | null;
  };
  performance: { disks: Performance[]; error: string | null };
  job: Job | null;
  engine: { available: boolean; name: string; native?: boolean };
  profiles: Record<string, ProfileRow[]>;
  minimise_to_tray: boolean;
  read_only_guard?: boolean;
  auto_check_updates?: boolean;
  health_mode?: string;
  language?: "tr" | "en";
  optimizations?: OptimizationRecord[];
  tier_moves?: TierMoveRecord[];
  version: string;
}
