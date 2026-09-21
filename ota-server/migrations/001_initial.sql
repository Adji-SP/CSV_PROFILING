CREATE TABLE IF NOT EXISTS projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                target TEXT NOT NULL,
                version TEXT NOT NULL,
                path TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

CREATE TABLE IF NOT EXISTS builds (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                status TEXT NOT NULL,
                logs_text TEXT NOT NULL DEFAULT '',
                exit_code INTEGER,
                error TEXT,
                started_at TEXT,
                finished_at TEXT,
                created_at TEXT NOT NULL,
                FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE CASCADE
            );

CREATE TABLE IF NOT EXISTS firmware (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                build_id TEXT NOT NULL UNIQUE,
                project TEXT NOT NULL,
                target TEXT NOT NULL,
                version TEXT NOT NULL,
                filename TEXT NOT NULL UNIQUE,
                path TEXT NOT NULL,
                file_size BIGINT NOT NULL,
                sha256 TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE RESTRICT,
                FOREIGN KEY(build_id) REFERENCES builds(id) ON DELETE RESTRICT
            );

CREATE TABLE IF NOT EXISTS devices (
                device_id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                chip TEXT NOT NULL,
                ip_address TEXT NOT NULL,
                current_version TEXT NOT NULL,
                desired_version TEXT,
                desired_firmware_id TEXT,
                status TEXT NOT NULL,
                ota_status TEXT NOT NULL,
                ota_progress BIGINT NOT NULL DEFAULT 0,
                ota_error TEXT,
                last_seen TEXT NOT NULL,
                FOREIGN KEY(desired_firmware_id) REFERENCES firmware(id) ON DELETE RESTRICT
            );

CREATE TABLE IF NOT EXISTS deployments (
                id TEXT PRIMARY KEY,
                device_id TEXT NOT NULL,
                firmware_id TEXT NOT NULL,
                status TEXT NOT NULL,
                progress BIGINT NOT NULL DEFAULT 0,
                error TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                FOREIGN KEY(device_id) REFERENCES devices(device_id) ON DELETE CASCADE,
                FOREIGN KEY(firmware_id) REFERENCES firmware(id) ON DELETE RESTRICT
            );

CREATE INDEX IF NOT EXISTS idx_builds_project ON builds(project_id);

CREATE INDEX IF NOT EXISTS idx_firmware_target_created ON firmware(target, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_deployments_device ON deployments(device_id, created_at DESC);
