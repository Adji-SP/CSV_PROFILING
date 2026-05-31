"""
Flask backend for CSV Profiler
- Sidecar JSON saved alongside each report (persistence fix)
- GET  /sessions               → list all past sessions from disk
- GET  /session/<job_id>       → check existence + return full sidecar metadata
- POST /clean/<job_id>         → apply cleaning operations, save cleaned CSV
- GET  /download-cleaned/<job_id> → serve cleaned CSV
- DELETE /reset/<job_id>       → remove one session
- DELETE /reset-all            → remove all sessions
"""

import json
import uuid
import queue
import threading
import traceback
from datetime import datetime, timezone
from pathlib import Path

from flask import Flask, request, jsonify, Response, send_file
from flask_cors import CORS
import pandas as pd
from ydata_profiling import ProfileReport

# ──────────────────────────────────────────────
# Configuration
# ──────────────────────────────────────────────
BASE_DIR   = Path(__file__).parent
UPLOAD_DIR = BASE_DIR / "uploads"
REPORT_DIR = BASE_DIR / "reports"
WEB_DIR    = BASE_DIR.parent / "web"

UPLOAD_DIR.mkdir(exist_ok=True)
REPORT_DIR.mkdir(exist_ok=True)

app = Flask(__name__, static_folder=str(WEB_DIR), static_url_path="")
CORS(app)

progress_queues: dict[str, queue.Queue] = {}

# ──────────────────────────────────────────────
# Helpers
# ──────────────────────────────────────────────
def sse_message(data: dict) -> str:
    return f"data: {json.dumps(data)}\n\n"


def sidecar_path(job_id: str) -> Path:
    return REPORT_DIR / f"{job_id}.json"


def save_sidecar(job_id: str, data: dict):
    with open(sidecar_path(job_id), "w", encoding="utf-8") as f:
        json.dump(data, f, ensure_ascii=False)


def load_sidecar(job_id: str) -> dict | None:
    p = sidecar_path(job_id)
    if not p.exists():
        return None
    try:
        with open(p, encoding="utf-8") as f:
            return json.load(f)
    except Exception:
        return None


def compute_metadata(df: pd.DataFrame) -> dict:
    """Analyse a DataFrame and return a metadata dict for the cleaning panel."""
    rows, cols = df.shape

    missing_info = df.isnull().sum()
    missing_cols = {c: int(v) for c, v in missing_info.items() if v > 0}

    duplicate_rows  = int(df.duplicated().sum())
    numeric_cols    = df.select_dtypes(include="number").columns.tolist()
    categorical_cols= df.select_dtypes(include=["object", "category"]).columns.tolist()
    constant_cols   = [c for c in df.columns if df[c].nunique() <= 1]
    datetime_cols   = df.select_dtypes(include=["datetime64"]).columns.tolist()

    # Detect datetime-like and numeric-like string columns
    numeric_like_cols = []
    for c in categorical_cols:
        try:
            # Try parsing as datetime; errors='raise' so we catch failures
            pd.to_datetime(df[c], errors="raise")
            if c not in datetime_cols:
                datetime_cols.append(c)
        except Exception:
            pass
        # Check if values are numeric
        converted = pd.to_numeric(df[c], errors="coerce")
        if converted.notna().mean() > 0.9 and c not in numeric_like_cols:
            numeric_like_cols.append(c)

    # Outlier detection via IQR
    outlier_cols = []
    for c in numeric_cols:
        col_data = df[c].dropna()
        if len(col_data) > 10:
            Q1, Q3 = col_data.quantile(0.25), col_data.quantile(0.75)
            IQR = Q3 - Q1
            if IQR > 0 and ((col_data < Q1 - 1.5 * IQR) | (col_data > Q3 + 1.5 * IQR)).any():
                outlier_cols.append(c)

    return {
        "rows":             rows,
        "cols":             cols,
        "missing_cols":     missing_cols,        # {colname: count}
        "duplicate_rows":   duplicate_rows,
        "numeric_cols":     numeric_cols,
        "categorical_cols": categorical_cols,
        "constant_cols":    constant_cols,
        "datetime_cols":    datetime_cols,
        "numeric_like_cols":numeric_like_cols,   # object cols that look numeric
        "outlier_cols":     outlier_cols,
        "all_cols":         df.columns.tolist(),
    }


# ──────────────────────────────────────────────
# Profiling worker
# ──────────────────────────────────────────────
def run_profiling(job_id: str, csv_path: Path, report_path: Path, original_filename: str):
    q = progress_queues[job_id]

    try:
        q.put(sse_message({"progress": 5, "status": "Reading CSV file…"}))
        df = pd.read_csv(csv_path)
        rows, cols = df.shape
        q.put(sse_message({
            "progress": 15,
            "status": f"Loaded {rows:,} rows × {cols} columns. Analysing…",
        }))

        profiling_done = threading.Event()
        profile_holder = {}

        def _profile_worker():
            try:
                profile = ProfileReport(df, title="CSV Profiling Report",
                                        explorative=True, progress_bar=False)
                profile.to_file(report_path)
                profile_holder["ok"] = True
            except Exception as exc:
                profile_holder["error"] = str(exc)
                profile_holder["tb"]    = traceback.format_exc()
            finally:
                profiling_done.set()

        threading.Thread(target=_profile_worker, daemon=True).start()

        ticker_steps = [
            (25, "Preparing dataset…"),
            (40, "Generating profiling report…"),
            (60, "Rendering EDA sections…"),
            (78, "Writing report file…"),
            (92, "Preparing report viewer…"),
            (97, "Almost done…"),
        ]
        for progress, label in ticker_steps:
            if profiling_done.wait(timeout=3):
                break
            q.put(sse_message({"progress": progress, "status": label}))

        profiling_done.wait()

        if "error" in profile_holder:
            q.put(sse_message({"progress": 100, "status": "Error during profiling.",
                               "error": profile_holder["error"]}))
            return

        metadata = compute_metadata(df)

        # ── Save sidecar JSON (persistence fix) ────────────────
        sidecar = {
            "job_id":      job_id,
            "filename":    original_filename,
            "report_url":  f"/report/{job_id}",
            "created_at":  datetime.now(timezone.utc).isoformat(),
            "metadata":    metadata,
        }
        save_sidecar(job_id, sidecar)

        q.put(sse_message({
            "progress":   100,
            "status":     "Report ready!",
            "report_url": f"/report/{job_id}",
            "metadata":   metadata,
        }))

    except Exception as exc:
        q.put(sse_message({"progress": 100, "status": "Unexpected error.",
                           "error": str(exc), "tb": traceback.format_exc()}))
    finally:
        q.put(None)


def active_csv_path(job_id: str) -> Path:
    cleaned = UPLOAD_DIR / f"{job_id}_cleaned.csv"
    return cleaned if cleaned.exists() else UPLOAD_DIR / f"{job_id}.csv"


def clean_json_value(value):
    if pd.isna(value):
        return None
    if hasattr(value, "isoformat"):
        return value.isoformat()
    return value.item() if hasattr(value, "item") else value


def dataframe_payload(df: pd.DataFrame, limit: int = 50) -> dict:
    view = df.head(max(1, min(int(limit or 50), 500))).copy()
    rows = []
    for idx, row in view.iterrows():
        item = {"__index": clean_json_value(idx)}
        for col in view.columns:
            item[col] = clean_json_value(row[col])
        rows.append(item)
    return {
        "columns": ["__index", *map(str, view.columns.tolist())],
        "rows": rows,
        "shape": list(df.shape),
    }


def parse_csv_list(raw: str) -> list[str]:
    return [part.strip() for part in str(raw or "").split(",") if part.strip()]


def coerce_series_value(series: pd.Series, raw: str):
    if pd.api.types.is_numeric_dtype(series):
        return pd.to_numeric(pd.Series([raw]), errors="coerce").iloc[0]
    if pd.api.types.is_datetime64_any_dtype(series):
        return pd.to_datetime(raw, errors="coerce")
    return raw


def refresh_dataset_report(job_id: str, df: pd.DataFrame) -> dict:
    cleaned_path = UPLOAD_DIR / f"{job_id}_cleaned.csv"
    report_path = REPORT_DIR / f"{job_id}.html"
    df.to_csv(cleaned_path, index=False)
    ProfileReport(df, title="CSV Profiling Report", explorative=True, progress_bar=False).to_file(report_path)

    updated_at = datetime.now(timezone.utc).isoformat()
    metadata = compute_metadata(df)
    sidecar = load_sidecar(job_id) or {}
    save_sidecar(job_id, {
        **sidecar,
        "job_id": job_id,
        "filename": sidecar.get("filename", f"{job_id}.csv"),
        "report_url": f"/report/{job_id}",
        "created_at": sidecar.get("created_at", updated_at),
        "cleaned_at": updated_at,
        "metadata": metadata,
    })
    return {"metadata": metadata, "cleaned_at": updated_at, "report_url": f"/report/{job_id}"}


# ──────────────────────────────────────────────
# Routes
# ──────────────────────────────────────────────

@app.route("/")
def index():
    return send_file(WEB_DIR / "index.html")


@app.route("/upload", methods=["POST"])
def upload():
    if "file" not in request.files:
        return jsonify({"error": "No file provided"}), 400
    f = request.files["file"]
    if not f.filename.lower().endswith(".csv"):
        return jsonify({"error": "Only CSV files are supported"}), 400

    job_id      = uuid.uuid4().hex
    csv_path    = UPLOAD_DIR / f"{job_id}.csv"
    report_path = REPORT_DIR / f"{job_id}.html"
    original_filename = f.filename

    f.save(csv_path)

    q = queue.Queue()
    progress_queues[job_id] = q

    threading.Thread(target=run_profiling,
                     args=(job_id, csv_path, report_path, original_filename),
                     daemon=True).start()

    return jsonify({"job_id": job_id})


@app.route("/progress/<job_id>")
def progress(job_id: str):
    if job_id not in progress_queues:
        return jsonify({"error": "Unknown job"}), 404
    q = progress_queues[job_id]

    def generate():
        while True:
            msg = q.get()
            if msg is None:
                break
            yield msg

    return Response(generate(), mimetype="text/event-stream",
                    headers={"Cache-Control": "no-cache", "X-Accel-Buffering": "no"})


@app.route("/report/<job_id>")
def report(job_id: str):
    report_path = REPORT_DIR / f"{job_id}.html"
    if not report_path.exists():
        return "Report not found", 404
    return send_file(report_path)


@app.route("/sessions")
def list_sessions():
    """Return all sessions — both sidecar JSON and HTML-only (no sidecar) reports."""
    sessions = {}

    # First pass: load from sidecar JSON files
    for p in REPORT_DIR.glob("*.json"):
        try:
            with open(p, encoding="utf-8") as f:
                data = json.load(f)
            jid = data.get("job_id") or p.stem
            html = REPORT_DIR / f"{jid}.html"
            if html.exists():
                sessions[jid] = {
                    "job_id":     jid,
                    "filename":   data.get("filename", "unknown.csv"),
                    "report_url": data.get("report_url", f"/report/{jid}"),
                    "created_at": data.get("created_at", ""),
                    "rows":       (data.get("metadata") or {}).get("rows"),
                    "cols":       (data.get("metadata") or {}).get("cols"),
                }
        except Exception:
            pass

    # Second pass: pick up HTML-only reports (no sidecar)
    for html in REPORT_DIR.glob("*.html"):
        jid = html.stem
        if jid not in sessions:
            mtime = html.stat().st_mtime
            sessions[jid] = {
                "job_id":     jid,
                "filename":   "unknown.csv",
                "report_url": f"/report/{jid}",
                "created_at": datetime.fromtimestamp(mtime, tz=timezone.utc).isoformat(),
                "rows":       None,
                "cols":       None,
            }

    # Sort newest first
    result = sorted(sessions.values(),
                    key=lambda x: x["created_at"],
                    reverse=True)
    return jsonify(result)


@app.route("/session/<job_id>")
def session_check(job_id: str):
    """Return sidecar metadata for a job if the report exists."""
    report_path = REPORT_DIR / f"{job_id}.html"
    if not report_path.exists():
        return jsonify({"exists": False})
    sc = load_sidecar(job_id)
    if sc:
        return jsonify({"exists": True, **sc})
    return jsonify({"exists": True, "report_url": f"/report/{job_id}", "metadata": None})


@app.route("/clean/<job_id>", methods=["POST"])
def clean(job_id: str):
    """Apply cleaning operations to the uploaded CSV and save cleaned version."""
    csv_path = UPLOAD_DIR / f"{job_id}.csv"
    if not csv_path.exists():
        return jsonify({"error": "Original CSV not found. Cannot apply cleaning."}), 404

    cleaned_path = UPLOAD_DIR / f"{job_id}_cleaned.csv"
    working_path = cleaned_path if cleaned_path.exists() else csv_path

    ops    = request.get_json(silent=True) or {}
    df     = pd.read_csv(working_path)
    before = df.shape
    changes = []

    # 1. Drop duplicates
    if ops.get("drop_duplicates"):
        subset = ops.get("duplicate_subset") or None   # list of cols or None = all
        before_rows = len(df)
        df = df.drop_duplicates(subset=subset)
        removed = before_rows - len(df)
        if removed:
            changes.append(f"Removed {removed} duplicate rows")

    # 2. Missing values – per column strategy
    missing_ops = ops.get("missing", {})   # {col: strategy}
    for col, strategy in missing_ops.items():
        if col not in df.columns:
            continue
        if strategy == "drop_rows":
            before_rows = len(df)
            df = df.dropna(subset=[col])
            changes.append(f"Dropped {before_rows - len(df)} rows where '{col}' is missing")
        elif strategy == "fill_median":
            val = df[col].median()
            df[col] = df[col].fillna(val)
            changes.append(f"Filled '{col}' missing with median ({val:.4g})")
        elif strategy == "fill_mode":
            val = df[col].mode()
            if not val.empty:
                df[col] = df[col].fillna(val[0])
                changes.append(f"Filled '{col}' missing with mode ({val[0]})")
        elif strategy == "fill_zero":
            df[col] = df[col].fillna(0)
            changes.append(f"Filled '{col}' missing with 0")
        elif strategy == "fill_unknown":
            df[col] = df[col].fillna("Unknown")
            changes.append(f"Filled '{col}' missing with 'Unknown'")
        elif strategy == "fill_mean":
            val = df[col].mean()
            df[col] = df[col].fillna(val)
            changes.append(f"Filled '{col}' missing with mean ({val:.4g})")

    # 3. Drop columns
    drop_cols = [c for c in ops.get("drop_cols", []) if c in df.columns]
    if drop_cols:
        df = df.drop(columns=drop_cols)
        changes.append(f"Dropped columns: {', '.join(drop_cols)}")

    # 4. Fix data types
    fix_types = ops.get("fix_types", {})  # {col: "datetime" | "numeric" | "string"}
    for col, target in fix_types.items():
        if col not in df.columns:
            continue
        try:
            if target == "datetime":
                df[col] = pd.to_datetime(df[col], errors="coerce")
                changes.append(f"Converted '{col}' to datetime")
            elif target == "numeric":
                df[col] = pd.to_numeric(df[col], errors="coerce")
                changes.append(f"Converted '{col}' to numeric")
            elif target == "string":
                df[col] = df[col].astype(str)
                changes.append(f"Converted '{col}' to string")
        except Exception as e:
            changes.append(f"Could not convert '{col}': {e}")

    # 5. Numerical formatting
    num_fmt = ops.get("numeric_format", {})  # {method: "normalize"|"standardize"|"round_N", cols: [...]}
    num_cols = num_fmt.get("cols", [])
    num_method = num_fmt.get("method", "none")
    for col in num_cols:
        if col not in df.columns:
            continue
        try:
            if num_method == "normalize":
                mn, mx = df[col].min(), df[col].max()
                if mx != mn:
                    df[col] = (df[col] - mn) / (mx - mn)
                    changes.append(f"Normalized '{col}' to [0, 1]")
            elif num_method == "standardize":
                mean, std = df[col].mean(), df[col].std()
                if std > 0:
                    df[col] = (df[col] - mean) / std
                    changes.append(f"Standardized '{col}' (mean=0, std=1)")
            elif num_method.startswith("round_"):
                decimals = int(num_method.split("_")[1])
                df[col] = df[col].round(decimals)
                changes.append(f"Rounded '{col}' to {decimals} decimal places")
        except Exception as e:
            changes.append(f"Numeric format error on '{col}': {e}")

    # 6. Cap outliers
    outlier_ops = ops.get("outliers", {})  # {cols: [...], method: "cap_iqr"|"remove_rows"}
    outlier_cols   = [c for c in outlier_ops.get("cols", []) if c in df.columns]
    outlier_method = outlier_ops.get("method", "cap_iqr")
    for col in outlier_cols:
        try:
            Q1 = df[col].quantile(0.25)
            Q3 = df[col].quantile(0.75)
            IQR = Q3 - Q1
            lo, hi = Q1 - 1.5 * IQR, Q3 + 1.5 * IQR
            if outlier_method == "cap_iqr":
                df[col] = df[col].clip(lo, hi)
                changes.append(f"Capped outliers in '{col}' to [{lo:.4g}, {hi:.4g}]")
            elif outlier_method == "remove_rows":
                before_rows = len(df)
                df = df[df[col].between(lo, hi)]
                changes.append(f"Removed {before_rows - len(df)} outlier rows from '{col}'")
        except Exception as e:
            changes.append(f"Outlier error on '{col}': {e}")

    # Save cleaned CSV.
    df.to_csv(cleaned_path, index=False)

    # Update sidecar metadata (no full re-profiling — that is slow and
    # the user can re-run profiling manually if a fresh EDA is needed).
    cleaned_at = datetime.now(timezone.utc).isoformat()
    metadata   = compute_metadata(df)
    sidecar    = load_sidecar(job_id) or {}
    save_sidecar(job_id, {
        **sidecar,
        "job_id":      job_id,
        "filename":    sidecar.get("filename", f"{job_id}.csv"),
        "report_url":  f"/report/{job_id}",
        "created_at":  sidecar.get("created_at", cleaned_at),
        "cleaned_at":  cleaned_at,
        "metadata":    metadata,
    })

    after = df.shape
    return jsonify({
        "ok":             True,
        "original_shape": list(before),
        "cleaned_shape":  list(after),
        "changes":        changes,
        "download_url":   f"/download-cleaned/{job_id}",
        "metadata":       metadata,
        "cleaned_at":     cleaned_at,
    })


@app.route("/playground/<job_id>", methods=["POST"])
def playground(job_id: str):
    csv_path = active_csv_path(job_id)
    if not csv_path.exists():
        return jsonify({"error": "Dataset not found for this session."}), 404

    payload = request.get_json(silent=True) or {}
    op = payload.get("operation", "preview")
    mutate = bool(payload.get("mutate"))
    limit = max(1, min(int(payload.get("limit") or 20), 500))
    column = (payload.get("column") or "").strip()
    column_2 = (payload.get("column_2") or "").strip()
    value = payload.get("value", "")
    operator = payload.get("operator", "eq")
    chart_type = payload.get("chart_type", "bar")

    df = pd.read_csv(csv_path)
    result_df = df.copy()
    changes = []
    chart = None

    try:
        if op == "head":
            result_df = df.head(limit)
        elif op == "tail":
            result_df = df.tail(limit)
        elif op == "sample":
            result_df = df.sample(n=min(limit, len(df)), random_state=42)
        elif op == "describe":
            desc = df.describe(include="all").reset_index().rename(columns={"index": "stat"})
            return jsonify({"ok": True, "table": dataframe_payload(desc, 500)})
        elif op == "value_counts":
            if column not in df.columns:
                return jsonify({"error": "Column is required for value_counts."}), 400
            vc = df[column].value_counts(dropna=False).head(limit).reset_index()
            vc.columns = [column, "count"]
            return jsonify({"ok": True, "table": dataframe_payload(vc, limit)})
        elif op == "sort_values":
            if column not in df.columns:
                return jsonify({"error": "Column is required for sort_values."}), 400
            result_df = df.sort_values(column, kind="mergesort").head(limit)
        elif op == "filter":
            if column not in df.columns:
                return jsonify({"error": "Column is required for filtering."}), 400
            compare_value = coerce_series_value(df[column], value)
            series = df[column]
            if operator == "contains":
                mask = series.astype(str).str.contains(str(value), case=False, na=False)
            elif operator == "ne":
                mask = series != compare_value
            elif operator == "gt":
                mask = series > compare_value
            elif operator == "gte":
                mask = series >= compare_value
            elif operator == "lt":
                mask = series < compare_value
            elif operator == "lte":
                mask = series <= compare_value
            else:
                mask = series == compare_value
            result_df = df[mask].head(limit)
        elif op == "drop_columns":
            cols = [c for c in parse_csv_list(column) if c in result_df.columns]
            if not cols:
                return jsonify({"error": "Provide one or more existing columns in Column / X."}), 400
            result_df = result_df.drop(columns=cols)
            changes.append(f"Dropped columns: {', '.join(cols)}")
        elif op == "drop_rows":
            positions = [int(x) for x in parse_csv_list(payload.get("indexes"))]
            valid = [pos for pos in positions if 0 <= pos < len(result_df)]
            result_df = result_df.drop(result_df.index[valid])
            changes.append(f"Dropped {len(valid)} rows by index position")
        elif op == "set_cell":
            if column not in result_df.columns:
                return jsonify({"error": "Column is required for set cell."}), 400
            positions = [int(x) for x in parse_csv_list(payload.get("indexes"))]
            if not positions:
                return jsonify({"error": "Provide a row index for set cell."}), 400
            pos = positions[0]
            if pos < 0 or pos >= len(result_df):
                return jsonify({"error": "Row index is out of range."}), 400
            result_df.iat[pos, result_df.columns.get_loc(column)] = coerce_series_value(result_df[column], value)
            changes.append(f"Set row {pos}, column '{column}'")
        elif op == "fillna":
            if column not in result_df.columns:
                return jsonify({"error": "Column is required for fillna."}), 400
            result_df[column] = result_df[column].fillna(coerce_series_value(result_df[column], value))
            changes.append(f"Filled missing values in '{column}'")
        elif op == "rename_column":
            if column not in result_df.columns or not column_2:
                return jsonify({"error": "Provide existing column and new name."}), 400
            result_df = result_df.rename(columns={column: column_2})
            changes.append(f"Renamed '{column}' to '{column_2}'")
        elif op == "astype":
            if column not in result_df.columns or column_2 not in {"string", "numeric", "datetime"}:
                return jsonify({"error": "Provide column and dtype: string, numeric, or datetime."}), 400
            if column_2 == "numeric":
                result_df[column] = pd.to_numeric(result_df[column], errors="coerce")
            elif column_2 == "datetime":
                result_df[column] = pd.to_datetime(result_df[column], errors="coerce")
            else:
                result_df[column] = result_df[column].astype(str)
            changes.append(f"Converted '{column}' to {column_2}")
        elif op == "chart":
            if chart_type == "hist":
                if column not in df.columns:
                    return jsonify({"error": "Column is required for histogram."}), 400
                values = pd.to_numeric(df[column], errors="coerce").dropna()
                bins = max(3, min(limit, 50))
                counts = pd.cut(values, bins=bins).value_counts(sort=False)
                chart = {"type": "bar", "labels": [str(i) for i in counts.index], "data": [int(v) for v in counts.values], "label": column}
            elif chart_type in {"line", "scatter"}:
                if column not in df.columns or column_2 not in df.columns:
                    return jsonify({"error": "Column / X and Y are required for line or scatter charts."}), 400
                view = df[[column, column_2]].dropna().head(limit)
                chart = {"type": chart_type, "labels": [clean_json_value(v) for v in view[column]], "data": [clean_json_value(v) for v in view[column_2]], "label": column_2}
            else:
                if column not in df.columns:
                    return jsonify({"error": "Column is required for bar chart."}), 400
                vc = df[column].value_counts(dropna=False).head(limit)
                chart = {"type": "bar", "labels": [str(v) for v in vc.index], "data": [int(v) for v in vc.values], "label": column}
        elif op == "preview":
            result_df = df.head(limit)
        else:
            return jsonify({"error": f"Unsupported operation: {op}"}), 400
    except Exception as exc:
        return jsonify({"error": str(exc)}), 400

    response = {
        "ok": True,
        "table": dataframe_payload(result_df, limit),
        "changes": changes,
        "chart": chart,
    }
    if mutate and op in {"drop_columns", "drop_rows", "set_cell", "fillna", "rename_column", "astype"}:
        response.update(refresh_dataset_report(job_id, result_df))
        response["download_url"] = f"/download-cleaned/{job_id}"
    return jsonify(response)


@app.route("/download-cleaned/<job_id>")
def download_cleaned(job_id: str):
    cleaned_path = UPLOAD_DIR / f"{job_id}_cleaned.csv"
    if not cleaned_path.exists():
        return "Cleaned file not found. Apply cleaning first.", 404
    sc       = load_sidecar(job_id)
    basename = sc.get("filename", "data.csv").replace(".csv", "") if sc else job_id
    return send_file(cleaned_path, as_attachment=True,
                     download_name=f"{basename}_cleaned.csv",
                     mimetype="text/csv")


@app.route("/reset/<job_id>", methods=["DELETE"])
def reset_session(job_id: str):
    for p in [UPLOAD_DIR / f"{job_id}.csv",
              UPLOAD_DIR / f"{job_id}_cleaned.csv",
              REPORT_DIR / f"{job_id}.html",
              REPORT_DIR / f"{job_id}.json"]:
        if p.exists():
            p.unlink()
    progress_queues.pop(job_id, None)
    return jsonify({"ok": True, "message": "Session cleared"})


@app.route("/reset-all", methods=["DELETE"])
def reset_all():
    deleted = 0
    for p in list(UPLOAD_DIR.glob("*.csv")) + list(REPORT_DIR.glob("*.html")) + list(REPORT_DIR.glob("*.json")):
        p.unlink(); deleted += 1
    progress_queues.clear()
    return jsonify({"ok": True, "message": f"Cleared {deleted} files"})


# ──────────────────────────────────────────────
# Entry point
# ──────────────────────────────────────────────
if __name__ == "__main__":
    print("=" * 55)
    print("  CSV Profiler Server  —  http://localhost:5000")
    print("=" * 55)
    app.run(host="0.0.0.0", port=5000, debug=False, threaded=True)
