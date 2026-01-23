---
title = "Cross-Platform Architecture & Local-First Synchronization"
authors = "Andrew Lyjak, Claude"
last_updated = "2025-11-14"
status = "Draft"
version = "0.1"
dependencies = ["dwelling_point_design.md (v0.1)", "intention_lattice.md (v0.1)"]
---

# Cross-Platform Architecture & Local-First Synchronization

## Purpose

This document defines Noet's cross-platform architecture using Flutter + rinf (Rust in Flutter) to deliver a unified application across iOS, Android, macOS, Windows, and Linux. Our architecture prioritizes:

1. **Native platform access** where required (dwelling point detection, sensors)
2. **Local-first synchronization** respecting participant data sovereignty
3. **Maximum code reuse** via single Flutter codebase + shared Rust core
4. **Zero data duplication** - single source of truth in Rust, no JSON IPC overhead

This document specifies our implementation approach and the technical decisions that support a sustainable, privacy-first, cross-platform system.

---

## Core Constraints

### 1. Platform-Specific Privileges

**Android Requirements:**
- `ForegroundService` with `FOREGROUND_SERVICE_LOCATION` permission
- Background sensor access (WiFi, BLE RSSI, GPS, accelerometer)
- Power management exemptions for long-running ML inference
- Notification channel for persistent foreground notification

**iOS Requirements:**
- Background location updates (limited compared to Android)
- **CoreLocation** for GPS/WiFi/BLE scanning
- **Significant location changes** API (coarser than Android)
- Background execution severely limited (10 min task limit)
- **Restrictions:** WiFi RSSI data access limited/deprecated since iOS 13, BLE RSSI more accessible but battery-constrained

**Desktop (macOS/Windows/Linux):**
- No ForegroundService equivalent
- Location services optional (not primary use case)
- Focus on authoring, visualization, review workflows
- BYOLLM integration for Socratic prompts

**Implication:** Dwelling point detection requires **native mobile apps** (Android primary, iOS compromised). Desktop cannot run dwelling point detection reliably.

---

### 2. Local-First Data Sovereignty

**Principles (from financial_analysis.md):**
- Participant data never leaves their control
- No cloud storage of sensitive data (activity logs, location, lattice)
- Synchronization must work peer-to-peer or via participant-controlled servers
- System must function 100% offline (sync is convenience, not requirement)

**Implication:** Cannot use traditional cloud sync services (Firebase, AWS Amplify). Must implement CRDT-based or operational transform-based sync.

---

### 3. Code Reuse via Rust Core

**Current Architecture:**
- `rust_core/crates/core/` - BeliefSet, codec, query engine (platform-agnostic)
- `rust_core/crates/dp_inf/` - Dwelling point inference (platform-agnostic ML)
- `rust_core/crates/db/` - SQLite database layer (platform-agnostic)
- `rust_core/crates/ffi/` - UniFFI bindings (Android, iOS, WASM)

**Benefit:** 80%+ of business logic is shared across platforms.

**Implication:** Platform-specific code should be thin "drivers" that:
- Collect sensor data (mobile)
- Render UI (platform-native or web)
- Orchestrate Rust core libraries

---

### 4. Synchronization Requirements

**Data Types to Sync:**

1. **All Schemas (Git-based version control)**
   
   **Intention Lattice** (`participant/intentions/`):
   - Dimensions, aspirations, goals, actions (participant-authored)
   - Relationship profiles (connection types + intensities)
   - Example: `participant/intentions/aspirations/asp-embodiment.toml`
   
   **Curriculum Content** (`participant/curriculum/`):
   - Lesson content and progression
   - Custom lessons authored by participant
   - Example: `participant/curriculum/lessons/lesson_1_noticing.toml`
   
   **Socratic Prompts** (`participant/socrates/`):
   - Custom prompt templates
   - Participant-specific prompt refinements
   - Example: `participant/socrates/prompts/daily_reflection.toml`
   
   **Templates** (`participant/template/`):
   - Reusable action templates
   - Goal/aspiration templates
   - Community-contributed templates
   
   **Training Plans** (post-MVP):
   - Choreographer-generated plans
   - Milestones and scheduled activities
   - Example: `participant/plans/training_plan_001.toml`
   
   **Sync Mechanism:** Git (push/pull to GitHub/GitLab)
   - Updates: Weekly-monthly (participant editing)
   - Conflicts: Git merge conflicts (human-resolved via UI)
   - **Benefits:**
     - Built-in backup (remote repository)
     - Community sharing (public repos for templates/libraries/curricula)
     - Version history (see evolution of lattice and customizations)
     - Collaboration (fork/contribute workflow for shared schemas)
     - Offline-first (local Git commits, push when online)

2. **Activity Logs** (Automerge CRDT - post-beta, if demanded)
   - Dwelling point visits (inferred from sensors)
   - Manual activity entries (participant logging)
   - Quality ratings (focus, energy)
   - Surprises (Active Inference markers)
   - **Sync mechanism:** Automerge (SQLite database, synced via CRDT)
   - Updates: Continuous (every 5-30 minutes on mobile)
   - Conflicts: Minimal (mostly append-only, timestamp-ordered)
   - **Not in Git:** Activity logs are streaming sensor data, not text files

3. **Visualizations & Derived Data** (computed, not synced)
   - Constellation views, glyphs, temporal scores
   - Computed on-device from lattice + activity logs
   - No sync required (ephemeral)

**Sync Patterns:**
- **All Schemas (Git):**
  - Desktop edits schema files → `git commit` → `git push` to GitHub/GitLab
  - Mobile pulls latest → `git pull` on app launch or manual refresh
  - Conflicts resolved in editor (merge conflict UI)
  - Public repos enable community sharing:
    - Starter lattices (example intention structures)
    - Action libraries (reusable actions for common activities)
    - Curriculum modules (community-contributed lessons)
    - Prompt templates (Socratic conversation starters)
  
- **Activity Logs (Automerge - post-beta):**
  - Mobile → Desktop: Activity logs generated on phone, synced via Automerge
  - Mobile ↔ Mobile: Multi-phone users merge logs via Automerge
  
- **Backup:**
  - All schemas: Backed up to Git remote (GitHub/GitLab)
  - Activity logs: Export SQLite DB (manual) or Automerge sync state

---

## Implemented Architecture: Flutter + rinf

### Technology Stack

- **All Platforms:** Flutter (single codebase for iOS, Android, macOS, Windows, Linux)
- **Rust Integration:** rinf (Rust in Flutter) - FFI with Protobuf messages, no JSON serialization
- **Dwelling Point Detection:** Native platform channels for ForegroundService (Android) / CoreLocation (iOS)
- **Visualizations:** d4 (Dart port of d3.js) as primary, flutter_inappwebview (WebView) as fallback
- **Git Integration:** git_bindings Flutter package for lattice version control
- **Future Sync:** Automerge CRDT for activity logs (post-beta, if demanded)

### Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                    Flutter UI Layer (Dart)                   │
│  • Single codebase for all platforms                        │
│  • Curriculum, lattice editor, visualizations                │
│  • d4 for native visualizations OR WebView for d3.js        │
│  • Material/Cupertino widgets                                │
└────────────────────────┬────────────────────────────────────┘
                         │
                         │ rinf FFI (Protobuf messages)
                         │ No JSON, zero-copy where possible
                         ▼
┌─────────────────────────────────────────────────────────────┐
│              Rust Core (rust_core/crates/)                   │
│  • compiler.rs: LatticeService, BeliefSet operations         │
│  • commands.rs: Op enum exposed via rinf                     │
│  • db: SQLite database layer                                 │
│  • dp_inf: Dwelling point inference (ML pipeline)            │
│  • Single source of truth - no data duplication             │
└────────────────────────┬────────────────────────────────────┘
                         │
                         │ Platform Channels (for privileged access)
                         ▼
┌─────────────────────────────────────────────────────────────┐
│              Platform-Specific Drivers                       │
│  • Android: ForegroundService (Kotlin, existing)             │
│  • iOS: CoreLocation (Swift, to be implemented)              │
│  • Desktop: No dwelling point detection                      │
└─────────────────────────────────────────────────────────────┘
```

### Data Flow

1. **UI → Rust (Query):**
   ```dart
   // Flutter UI needs to query lattice
   final response = await GetStatesRequest(
     query: PaginatedQuery(/* ... */),
   ).sendSignalToRust();
   
   // response.results is strongly-typed Dart object
   // No JSON parsing, no data duplication
   ```

2. **Rust → UI (Results):**
   ```rust
   // rust_core/crates/messages/src/lib.rs (rinf messages)
   pub async fn get_states(request: GetStatesRequest) -> GetStatesResponse {
       let lattice_service = LATTICE_SERVICE.lock().await;
       let results = lattice_service.get_states(request.query).await;
       GetStatesResponse { results }
   }
   ```

3. **Platform Sensors → Rust:**
   ```kotlin
   // Android ForegroundService collects sensor data
   val wifiScan = wifiManager.scanResults
   rustCore.onWifiScan(wifiScan) // Via platform channel
   ```

### Key Design Decisions

#### 1. Single Codebase for All Platforms

**Decision:** Use Flutter for unified UI across iOS, Android, macOS, Windows, Linux.

**Rationale:**
- 4-6 months to deliver all platforms vs. 15-21 months for separate native apps
- Brandy can author curriculum/content once, works everywhere
- Hot reload accelerates iteration velocity
- LLM assistance (Claude/Gemini) excellent with Flutter/Dart

**Implementation:**
- `lib/` directory contains shared Dart code
- Platform-specific code only for privileged operations (sensors)
- Responsive design adapts to mobile vs. desktop screen sizes

#### 2. No Data Duplication via rinf FFI

**Decision:** Use rinf (not UniFFI or Tauri) for Dart ↔ Rust communication.

**Rationale:**
- **No JSON IPC overhead:** Protobuf messages compiled to Rust + Dart types
- **Single source of truth:** BeliefSet lives in Rust, Dart receives query results only
- **Zero-copy where possible:** FFI passes pointers, not serialized data
- **Performance:** 10-100x faster than JSON serialization for large data structures

**Comparison to Alternatives:**
| Feature | rinf (chosen) | UniFFI | Tauri |
|---------|---------------|--------|-------|
| Message passing | Protobuf | JSON | JSON |
| Data duplication | None | Frontend cache | Frontend cache |
| FFI overhead | Minimal | Moderate | High |
| Platform support | Dart/Flutter only | Multi-language | Web frontend only |

**Trade-offs Accepted:**
- rinf is newer than UniFFI (less mature ecosystem)
- Protobuf schema definitions add build complexity
- Flutter-only (no path to React/Vue if we change UI framework)

#### 3. Markdown Rendering: Native Widgets

**Decision:** Use `markdown_widget` package for native Flutter markdown rendering (no WebView).

**Rationale:**
- **Native widgets:** Renders markdown directly to Flutter widget tree
- **Full feature set:** TOC, code highlighting, selectable text, dark mode
- **Actively maintained:** Verified publisher (morn.fun), recent updates
- **Performance:** Faster and more battery-efficient than WebView
- **Seamless integration:** Shares theme, fonts, styling with rest of app

**Implementation:**

**Curriculum Lessons:**
```dart
// lib/curriculum/lesson_view.dart
import 'package:markdown_widget/markdown_widget.dart';

class LessonView extends StatelessWidget {
  final String markdownContent;
  
  Widget build(BuildContext context) {
    return MarkdownWidget(
      data: markdownContent,
      tocController: TocController(), // Table of contents navigation
      config: MarkdownConfig(
        configs: [
          // Code block highlighting
          PreConfig(theme: atomOneLightTheme),
          // Custom link handling
          LinkConfig(onTap: (url) => handleLessonLink(url)),
        ],
      ),
    );
  }
}
```

**TOML Frontmatter Editing:**

Desktop (split editor):
```dart
Column(
  children: [
    // TOML editor with syntax highlighting
    Expanded(child: TomlEditor(controller: tomlController)),
    Divider(),
    // Markdown editor + preview
    Expanded(child: MarkdownWidget(data: mdController.text)),
  ],
)
```

Mobile (parsed form):
```dart
// Parse TOML → form fields, render markdown below
Column(
  children: [
    TextField(label: 'Title', value: doc.frontmatter['title']),
    TextField(label: 'Type', value: doc.frontmatter['type']),
    Divider(),
    Expanded(child: MarkdownWidget(data: doc.body)),
  ],
)
```

**Benefits:**
- No WebView dependency for core content rendering
- Better accessibility (native text selection, screen readers)
- Consistent architecture (avoid WebView unless absolutely necessary)

**Custom Rendering Requirements:**

Noet's markdown files use bespoke TOML frontmatter and require custom rendering for:

1. **Frontmatter Integration:**
   - Parse TOML frontmatter (title, type, metadata)
   - Display frontmatter data in UI (breadcrumbs, headers, metadata panels)
   - Example: `participant/intentions/aspirations/asp-embodiment.toml`

2. **Subsection Anchors:**
   - Extract subsection IDs from frontmatter
   - Generate anchor links for navigation
   - Example: `subsections = ["body", "movement", "rest"]` → clickable TOC

3. **Cross-Reference Links:**
   - Parse Noet-specific link syntax: `[@aspiration:asp-embodiment]`
   - Resolve to internal navigation or rendering
   - Distinguish from standard markdown links

**Implementation Strategy:**

Option A: Fork `markdown_widget` and extend:
```dart
// Custom MarkdownConfig with Noet-specific parsers
class NoetMarkdownWidget extends MarkdownWidget {
  final TomlFrontmatter frontmatter;
  
  NoetMarkdownWidget({
    required String data,
    required this.frontmatter,
  }) : super(
    data: data,
    config: MarkdownConfig(
      configs: [
        // Custom link parser for [@ref:id] syntax
        LinkConfig(
          onTap: (url) => handleNoetLink(url, frontmatter),
        ),
        // Custom heading parser with anchor IDs
        H1Config(
          id: (text) => frontmatter.subsections[text],
        ),
      ],
    ),
  );
}
```

Option B: Wrap `markdown_widget` with preprocessing:
```dart
class NoetDocumentRenderer extends StatelessWidget {
  final NoetDocument doc; // Parsed TOML + markdown
  
  Widget build(BuildContext context) {
    // Preprocess markdown: replace [@ref:id] with [text](noet://ref/id)
    final processedMarkdown = preprocessNoetSyntax(doc.body, doc.frontmatter);
    
    return Column(
      children: [
        // Render frontmatter as Flutter widgets
        DocumentHeader(frontmatter: doc.frontmatter),
        
        // Render markdown with custom link handler
        Expanded(
          child: MarkdownWidget(
            data: processedMarkdown,
            config: MarkdownConfig(
              configs: [
                LinkConfig(
                  onTap: (url) {
                    if (url.startsWith('noet://')) {
                      handleNoetNavigation(url);
                    } else {
                      launchUrl(url);
                    }
                  },
                ),
              ],
            ),
          ),
        ),
      ],
    );
  }
}
```

**Recommendation:** Start with Option B (preprocessing wrapper) to avoid forking. Only fork `markdown_widget` if preprocessing becomes too complex or performance-critical.

**Markdown Editing:**

For editing TOML frontmatter + markdown documents, use native TextField with optional enhancements:

**MVP Approach:**
```dart
class DocumentEditor extends StatefulWidget {
  Widget build(BuildContext context) {
    return Column(
      children: [
        // TOML frontmatter editor (collapsible)
        ExpansionTile(
          title: Text('Frontmatter'),
          children: [
            TextField(
              controller: _tomlController,
              maxLines: null,
              style: TextStyle(fontFamily: 'monospace', fontSize: 12),
              decoration: InputDecoration(
                hintText: 'title = "..."\ntype = "..."',
              ),
            ),
          ],
        ),
        
        // Edit/Preview tabs
        Expanded(
          child: TabBarView(
            children: [
              // Edit: Raw markdown
              TextField(
                controller: _markdownController,
                maxLines: null,
                style: TextStyle(fontFamily: 'monospace'),
              ),
              
              // Preview: Rendered with NoetMarkdownWidget
              SingleChildScrollView(
                child: NoetDocumentRenderer(doc: currentDoc),
              ),
            ],
          ),
        ),
      ],
    );
  }
}
```

**Future Enhancements (Post-MVP):**

1. **Syntax Highlighting:**
   - Use `flutter_highlight` or `code_text_field` packages
   - Apply markdown syntax highlighting in editor
   - Example: `**bold**` → styled differently

2. **Line Numbers:**
   - Use `code_text_field` package (includes line numbers)
   - Or custom implementation with `TextField` + overlay

3. **Markdown Toolbar:**
   - Add buttons for common operations: bold, italic, heading, link
   - Insert markdown syntax at cursor position
   ```dart
   IconButton(
     icon: Icon(Icons.format_bold),
     onPressed: () {
       final selection = _controller.selection;
       final text = _controller.text;
       final newText = text.replaceRange(
         selection.start,
         selection.end,
         '**${text.substring(selection.start, selection.end)}**',
       );
       _controller.value = TextEditingValue(
         text: newText,
         selection: TextSelection.collapsed(offset: selection.end + 2),
       );
     },
   )
   ```

4. **Split-Pane (Desktop):**
   - Left: Editor with syntax highlighting
   - Right: Live preview
   - Synchronized scrolling

**Relevant Packages for Future:**
- `flutter_highlight` - Syntax highlighting (actively maintained)
- `code_text_field` - Code editor with line numbers (actively maintained)
- `re_editor` - Advanced code editor (newer, less mature)

**Decision:** Start with plain TextField for MVP (Weeks 3-4), add syntax highlighting in Phase 3 (Weeks 17-24) if time permits.

#### 4. Visualization Strategy: d4 Primary, WebView Fallback

**Decision:** Use d4 (Dart port of d3.js) for native Flutter rendering, fallback to WebView if needed.

**Rationale:**
- **Best performance:** d4 renders to Flutter Canvas (60fps, no WebView overhead)
- **LLM-maintainable:** Package abandoned (2021) but we can fork and extend with Claude/Gemini
- **Acceptable fallback:** WebView performance adequate for read-only visualizations
- **Architectural consistency:** Since markdown uses native widgets, prefer native for visualizations too

**Implementation:**

**Phase 1: d4 Prototype (Week 2 of development)**
```dart
// lib/visualizations/constellation_view.dart
import 'package:d4/d4.dart';

class ConstellationView extends StatelessWidget {
  Widget build(BuildContext context) {
    return CustomPaint(
      painter: ConstellationPainter(nodes, edges),
    );
  }
}

class ConstellationPainter extends CustomPainter {
  void paint(Canvas canvas, Size size) {
    // d4 force simulation renders to Flutter Canvas
    final simulation = ForceSimulation(nodes)
      ..force('charge', ManyBody())
      ..force('link', Link(edges))
      ..force('center', Center(size.width / 2, size.height / 2));
    
    // Render nodes and edges
    // ...
  }
}
```

**Phase 2: WebView Fallback (if d4 blocked)**
```dart
// lib/visualizations/constellation_webview.dart
import 'package:flutter_inappwebview/flutter_inappwebview.dart';

class ConstellationWebView extends StatelessWidget {
  Widget build(BuildContext context) {
    final html = rustCore.generateConstellationHtml(); // Rust generates d3.js
    return InAppWebView(
      initialData: InAppWebViewInitialData(data: html),
    );
  }
}
```

**Decision Point:** After Week 2 prototype, choose d4 if:
- Force simulation works for 100+ nodes
- Pan/zoom interactions smooth (60fps)
- Can extend/fix with <1 week LLM-assisted effort

Otherwise, use WebView (acceptable for read-only views).

#### 5. Git-Based Schema Synchronization

**Decision:** Use Git (GitHub/GitLab) for version control and sync of all schemas except activity logs.

**Schemas Under Git Control:**
1. **Intention Lattice** (`participant/intentions/`)
   - Dimensions, aspirations, goals, actions
   - Relationship profiles (parent_connections)
   - Example: `participant/intentions/aspirations/asp-embodiment.toml`

2. **Curriculum Content** (`participant/curriculum/`)
   - Lesson content and progression
   - Custom lessons authored by participant
   - Example: `participant/curriculum/lessons/lesson_1_noticing.toml`

3. **Socratic Prompts** (`participant/socrates/`)
   - Custom prompt templates
   - Participant-specific prompt refinements
   - Example: `participant/socrates/prompts/daily_reflection.toml`

4. **Templates** (`participant/template/`)
   - Reusable action templates
   - Goal/aspiration templates
   - Community-contributed templates

**Not Under Git Control:**
- **Activity Logs** (SQLite database, synced via Automerge post-beta if demanded)
- **Dwelling Point Visits** (continuous sensor data)
- **Configuration State** (API keys, UI preferences)

**Rationale:**
- **Built-in backup:** Remote repository is automatic backup for all authored content
- **Version history:** Participants can see evolution of their lattice and customizations over time
- **Community sharing:** Public repos enable sharing starter lattices, curriculum modules, prompt templates, action libraries
- **Collaboration:** Fork/contribute workflow for shared schemas
- **Offline-first:** Local commits, push when online
- **Developer-friendly:** Participants who know Git get full power
- **Appropriate granularity:** Git is designed for text files (TOML/Markdown), not streaming sensor data

**Implementation:**

**Library:** git_bindings Flutter package (https://pub.dev/packages/git_bindings)
- FFI wrapper around libgit2 (native Git library)
- SSH key authentication via libssh2
- Platform support: Android, iOS, macOS, Windows, Linux

**Git Workflow:**

1. **Initial Setup (Onboarding):**
   ```dart
   // User creates GitHub/GitLab repo or forks template
   final repo = await GitRepo.clone(
     url: 'git@github.com:user/noet-lattice.git',
     localPath: '/path/to/lattices/user',
     credentials: SshCredentials(privateKey: userSshKey),
   );
   ```

2. **Daily Editing:**
   ```dart
   // Auto-commit on file save
   await repo.add('participant/intentions/aspirations.toml');
   await repo.commit('Update: aspirations.toml');
   ```

3. **Sync:**
   ```dart
   // "Sync Lattice" button → pull + push
   await repo.pull(); // Fetch + merge
   await repo.push(); // Upload commits
   ```

4. **Conflict Resolution:**
   ```dart
   // If merge conflict detected
   final conflicts = await repo.getConflicts();
   // Show UI: "Merge conflict in aspirations.toml. Resolve?"
   // Let participant edit file, then:
   await repo.add('participant/intentions/aspirations.toml');
   await repo.commit('Resolved merge conflict');
   ```

**SSH Key Management:**
- Generate ED25519 key pair on device (first launch)
- Store private key in secure storage (Keychain/KeyStore)
- Display public key for user to add to GitHub/GitLab
- Alternative: GitHub device authorization flow (OAuth)

**Community Features:**
- Noet maintains public repos: `noet-community/starter-lattice`, `noet-community/action-library`
- Participants fork and customize
- Contribute back via pull requests (optional)

**Limitations:**
- Learning curve for non-technical participants (mitigated by guided onboarding)
- Merge conflicts require understanding (mitigated by auto-merge where possible)
- SSH setup friction (mitigated by step-by-step wizard)

#### 6. Automerge for Activity Logs (Post-Beta)

**Decision:** Defer Automerge CRDT implementation until beta participants demand cross-device activity sync.

**Rationale:**
- **MVP:** Single device dwelling point detection sufficient
- **Complexity:** Automerge integration non-trivial (wrapping SQLite schema, sync protocol)
- **Demand-driven:** Beta testing will reveal if participants need multi-device activity sync

**Future Implementation (if demanded):**
- Add `rust_core/crates/sync/` with Automerge Rust library
- Wrap activity log SQLite tables in Automerge documents
- Expose sync operations via rinf to Flutter
- Implement mDNS discovery (Dart package: multicast_dns)
- Build sync UI: status indicators, conflict resolution

**Activity Logs Sync Workflow (Future):**
```dart
// Mobile generates activity logs, syncs to desktop
final syncManager = await SyncManager.init(); // Rust via rinf
await syncManager.startSync(); // mDNS discovery, Automerge sync
// Activity logs automatically merged (append-only, timestamp-ordered)
```

### Platform-Specific Implementation

#### Android (Primary Platform)

**Dwelling Point Detection:**
- Reuse existing ForegroundService implementation (Kotlin)
- Platform channel exposes sensor data to Flutter:
  ```dart
  // lib/platform/android_sensors.dart
  final sensorChannel = MethodChannel('noet/sensors');
  final wifiScan = await sensorChannel.invokeMethod('getWifiScan');
  // Pass to Rust via rinf
  await OnWifiScan(scan: wifiScan).sendSignalToRust();
  ```

**Privileges Required:**
- `FOREGROUND_SERVICE_LOCATION` permission
- Background sensor access (WiFi, BLE RSSI, GPS, accelerometer)
- Persistent notification (foreground service requirement)

**Status:** Milestones 1-5 complete (ForegroundService driver, Event Gatekeeper, sensor managers)

#### iOS (Compromised, Deferred)

**Dwelling Point Detection:**
- CoreLocation background location (Swift platform channel)
- Limited compared to Android:
  - WiFi RSSI deprecated (iOS 13+)
  - BLE scanning battery-constrained
  - GPS requires "Always Allow" (user hostile)
  - Significant location changes API (100-500m accuracy)

**Implementation Strategy:**
- **MVP:** Defer iOS until Android beta validates product-market fit
- **Post-Beta (if demanded):** iOS Lite with degraded dwelling point resolution
  - Use significant location changes API only
  - Lower resolution (neighborhood-level, not room-level)
  - Supplement with manual logging
  - Clearly communicate limitations

#### Desktop (macOS, Windows, Linux)

**No Dwelling Point Detection:**
- Desktop is for authoring, visualization, review workflows
- No ForegroundService equivalent
- Location services optional (not primary use case)

**Primary Use Cases:**
- Curriculum lesson reading
- Lattice editing (TOML files with live preview)
- Visualization review (constellation, temporal score, glyphs)
- BYOLLM Socratic prompts (OpenAI/Anthropic/Ollama)
- Git sync UI (pull/push to GitHub/GitLab)

**Platform Channels:** None required (pure Flutter + rinf)

### Development Phases

#### Phase 1: Desktop App Foundation (Weeks 1-8)

**Weeks 1-2: rinf Integration & Prototype**
- Set up Flutter desktop project (macOS/Windows/Linux)
- Integrate rinf with rust_core
- Expose commands.rs operations via Protobuf messages
- Prototype: Query lattice (GetStates), display in Flutter UI
- Validate: No data duplication, FFI latency <10ms

**Weeks 3-4: Curriculum & Lattice Editor**
- Build curriculum reader UI (Lessons 1-7 from participant/curriculum/)
- Implement lattice editor (TOML editing with live preview)
- TOML syntax highlighting and validation
- File tree navigation (dimensions, aspirations, goals, actions)

**Weeks 5-6: BYOLLM Integration**
- API key management UI (OpenAI, Anthropic, Ollama)
- Socratic prompt interface (calls Rust backend via rinf)
- Chat history display (markdown rendering)

**Weeks 7-8: Git Integration**
- git_bindings integration
- Clone/pull/push workflows
- SSH key generation and management
- Onboarding wizard (connect GitHub/GitLab)

**Deliverable:** Desktop app for Brandy's alpha testing (content authoring workflow)

#### Phase 2: Mobile App with Dwelling Point Detection (Weeks 9-16)

**Weeks 9-10: Flutter Mobile Setup**
- Flutter mobile project (Android + iOS)
- Platform channel for ForegroundService (reuse existing Kotlin)
- Platform channel for CoreLocation (new Swift code for iOS)
- Integrate rinf (same rust_core, 100% code reuse)

**Weeks 11-12: Mobile UI**
- Mobile curriculum UI (same Dart code as desktop, responsive layout)
- Activity log review screens (list view, detail view)
- Quick logging interface (add manual activity)
- Navigation (bottom bar, drawer)

**Weeks 13-14: SQLite Export/Import (Manual Sync)**
- Export SQLite database to Downloads folder
- Import SQLite database (file picker)
- Conflict warning UI ("Overwrite or merge?")

**Weeks 15-16: Mobile Testing**
- Physical device testing (Pixel, OnePlus, Samsung)
- Battery usage profiling
- ForegroundService reliability testing
- UX iteration based on dogfooding

**Deliverable:** Android app with dwelling point detection, manual sync to desktop

#### Phase 3: Visualizations & Beta Prep (Weeks 17-24)

**Weeks 17-18: d4 Prototype**
- Fork d4 package (https://pub.dev/packages/d4)
- Implement constellation view (force-directed layout)
- Test with 100+ nodes, pan/zoom interactions
- **Decision point:** If d4 works → continue. If blocked → fallback to WebView.

**Weeks 19-20: Visualizations Implementation**
- Constellation view (d4 OR WebView)
- Temporal score line chart (Flutter native charting OR d4)
- Glyph rendering (Flutter CustomPaint)
- Visualization state management (zoom level, selected nodes)

**Weeks 21-22: Performance Optimization**
- Profile visualization rendering (60fps target)
- Optimize FFI calls (batch queries where possible)
- Memory usage testing (ensure no leaks)
- Startup time optimization

**Weeks 23-24: Beta Onboarding**
- First-time user experience (OOBE)
- Curriculum progression tracking
- Socratic prompt onboarding
- Git setup wizard
- Beta feedback forms

**Deliverable:** Production-ready app for beta launch (Android + Desktop, manual sync)

#### Phase 4: Automerge Auto-Sync (Post-Beta, 6-12 months)

**Only if beta participants demand automatic activity log sync:**

**Weeks 1-4: Automerge Integration**
- Add `rust_core/crates/sync/` with Automerge library
- Wrap activity log SQLite tables in Automerge documents
- Expose sync operations via rinf to Flutter

**Weeks 5-8: Sync UI**
- mDNS discovery (Dart package: multicast_dns)
- Sync status indicators
- Conflict resolution UI (if needed)
- Background sync scheduling

**Weeks 9-12: Optional Relay Server**
- Deploy relay server for cross-network sync (paid tier)
- End-to-end encryption (relay is blind)
- Subscription management

**Deliverable:** Automatic cross-device activity log sync (optional paid tier)

---

## Architecture Options (Appendix)

### Option 1: Native Apps + Automerge CRDT Sync

**Technology Stack:**
- **Mobile:** Native Android (Kotlin) + iOS (Swift)
- **Desktop:** Native apps (Swift/SwiftUI for macOS, C#/Avalonia for Windows, Rust/GTK for Linux)
- **Sync:** Automerge CRDT library (Rust + platform bindings)
- **Data:** SQLite local storage + Automerge documents for sync

**How It Works:**

1. **Local Data:**
   - SQLite database on each device (intention lattice, activity logs)
   - Automerge document wraps critical data structures (lattice nodes, activity entries)
   - Rust core reads/writes to SQLite, Automerge tracks changes

2. **Sync Process:**
   - Devices exchange Automerge sync messages via:
     - **Local WiFi (peer-to-peer):** mDNS discovery, direct TCP connection
     - **Participant-controlled relay server:** Optional sync server (self-hosted or paid tier)
     - **Bluetooth sync:** For offline scenarios (mobile-to-mobile)
   - Automerge automatically merges concurrent edits with CRDT semantics
   - No central authority required (fully decentralized)

3. **Conflict Resolution:**
   - **Lattice edits:** Automerge handles concurrent updates (last-write-wins for scalars, set union for relationships)
   - **Activity logs:** Append-only, timestamp-ordered (no conflicts)
   - **Deletions:** Tombstone records (Automerge preserves causal history)

**Pros:**
- ✅ True local-first (no cloud dependency)
- ✅ Automerge production-ready (Ink & Switch endorsement)
- ✅ Native performance (no WebView overhead)
- ✅ Full platform API access (sensors, background services)
- ✅ Rust core shared across all platforms (via UniFFI)

**Cons:**
- ❌ Native UI development per platform (3-4 separate UIs)
- ❌ Automerge integration complexity (wrapping SQLite schema)
- ❌ Peer-to-peer discovery UX (how do devices find each other?)
- ❌ iOS dwelling point detection severely limited (CoreLocation restrictions)
- ❌ High development cost (3-4x compared to single web app)

**Estimated Effort:** 6-9 months per platform (Android primary, desktop second, iOS deferred)

---

### Option 2: Flutter + rinf (Rust in Flutter) - Universal Cross-Platform

**Technology Stack:**
- **All Platforms:** Flutter (single codebase for mobile + desktop + web)
- **Rust Integration:** rinf (Rust messages ↔ Dart via FFI, no JSON serialization)
- **Dwelling Point:** Native platform channels for ForegroundService (Android) / CoreLocation (iOS)
- **Visualizations:** flutter_inappwebview for d3.js (WebView-based)
- **Sync:** Automerge CRDT in Rust, exposed via rinf

**How It Works:**

1. **Flutter UI Layer:**
   - Single Dart codebase for all platforms (mobile, desktop, web)
   - Curriculum lessons, lattice editor, visualizations
   - d3.js visualizations via WebView (flutter_inappwebview)
   - Native UI widgets (Material/Cupertino) for forms, navigation

2. **Rust Core Integration (rinf):**
   - `rust_core` libraries exposed via rinf FFI
   - **No JSON IPC:** Dart ↔ Rust via Protobuf or direct FFI (zero-copy where possible)
   - BeliefSet queries, codec operations, lattice transformations all in Rust
   - **Single source of truth:** SQLite accessed only from Rust, Dart receives query results
   - Commands from commands.rs exposed directly to Dart

3. **Platform-Specific Code (Method Channels):**
   - Android: ForegroundService for dwelling point detection (Kotlin)
   - iOS: CoreLocation background location (Swift)
   - Rust `dp_inf` crate receives sensor data via rinf from platform code
   - Desktop: No dwelling point detection (authoring/review only)

4. **Data Flow:**
   ```
   Flutter UI (Dart)
        ↕ (rinf FFI, no JSON)
   Rust Core (compiler.rs, commands.rs)
        ↕ (SQLite, in-memory)
   BeliefSet / Codec / Query Engine
   ```

5. **Visualizations (Three Options):**
   
   **Option A: d4 (Dart port of d3) - Native Flutter Canvas**
   - d4 package: https://pub.dev/packages/d4
   - Pure Dart/Flutter rendering (no WebView)
   - SVG or Canvas output
   - **Status:** Last updated 2021, may be abandoned
   - **With LLM assistance:** Can fork and maintain/extend as needed
   - **Best performance:** No WebView overhead, native 60fps
   
   **Option B: WebView (flutter_inappwebview) - d3.js**
   - Rust generates HTML/JS for d3.js visualizations
   - Render in WebView widget
   - **Trade-off:** WebView overhead vs. mature d3.js ecosystem
   - **Acceptable for:** Read-only views (constellation, temporal score)
   
   **Option C: Flutter Custom Painters - Pure Dart**
   - CustomPaint widget with manual Canvas drawing
   - No dependencies (d4 or d3.js)
   - **Most work:** Implement force-directed layout, SVG path rendering manually
   - **Best control:** Full customization, best performance

**Pros:**
- ✅ **Single codebase** for mobile + desktop (iOS, Android, macOS, Windows, Linux)
- ✅ **No JSON IPC overhead** with rinf (direct FFI, Protobuf messages)
- ✅ **No data duplication** - Rust owns BeliefSet, Dart queries via FFI
- ✅ d3.js access via WebView for sophisticated visualizations
- ✅ Hot reload during development (Flutter's killer feature)
- ✅ Rust core integrated directly (compiler.rs, commands.rs used as-is)
- ✅ Native performance for critical paths (FFI is fast)
- ✅ Mature ecosystem (Flutter 3.x is production-ready)
- ✅ LLM-assisted development (Gemini/Claude excellent with Flutter)

**Cons:**
- ❌ Visualization strategy TBD (d4 abandoned? WebView overhead? CustomPaint effort?)
- ❌ Still need platform channels for ForegroundService (small Kotlin/Swift glue code)
- ❌ Flutter bundle size larger than native (~20-30MB vs ~5-10MB)
- ❌ Learning curve for rinf (newer than UniFFI, less documentation)
- ❌ Desktop dwelling point detection still not viable (limitation of platform, not Flutter)

**rinf vs UniFFI Comparison:**

| Feature | rinf | UniFFI |
|---------|------|--------|
| Message passing | Protobuf (zero-copy) | JSON (serialization overhead) |
| FFI calls | Direct Dart FFI | Generated bindings per language |
| Async support | ✅ Rust async → Dart Futures | ✅ Rust async → platform async |
| Platform support | Dart only (Flutter) | Kotlin, Swift, JS, Python |
| Maturity | Newer, growing | Mature (Mozilla project) |
| Documentation | Moderate | Excellent |
| Performance | Faster (no JSON) | Slower (JSON serialization) |

**rinf Integration Example:**

```rust
// rust_core/crates/messages/src/lib.rs (rinf messages)

// No JSON - Protobuf definitions compiled to Rust + Dart
message GetStatesRequest {
    PaginatedQuery query = 1;
}

message GetStatesResponse {
    ResultsPage results = 1;
}

// Rust handler
pub async fn get_states(request: GetStatesRequest) -> GetStatesResponse {
    let lattice_service = LATTICE_SERVICE.lock().await;
    let results = lattice_service.get_states(request.query).await;
    GetStatesResponse { results }
}
```

```dart
// Flutter (Dart) - calls Rust directly, no JSON
final response = await GetStatesRequest(
  query: PaginatedQuery(/* ... */),
).sendSignalToRust();

// response.results is strongly-typed Dart object
// No JSON parsing, no data duplication
```

**Estimated Effort:** 4-6 months for full Flutter + rinf implementation (all platforms)

---

### Option 2b: Tauri Desktop + Native Mobile + Automerge Sync (Original Option 2)

**Technology Stack:**
- **Mobile:** Native Android (Kotlin) + iOS (Swift) with dwelling point detection
- **Desktop:** Tauri (Rust backend + React frontend)
- **Sync:** Automerge CRDT library
- **Data:** SQLite on all platforms

**How It Works:**

1. **Desktop App (Tauri):**
   - React frontend for curriculum, lattice editor, visualizations
   - Rust backend integrates `rust_core` directly (commands.rs via Tauri invoke)
   - **JSON IPC overhead:** Frontend calls Rust via JSON serialization
   - **Data duplication risk:** Frontend may cache lattice state in JS for reactivity
   - BYOLLM integration via Rust backend (API key management)
   - **No dwelling point detection** (desktop is for authoring/review only)

2. **Mobile Apps (Native):**
   - Dwelling point detection via ForegroundService (Android) / CoreLocation (iOS)
   - Rust core via UniFFI for all business logic
   - Native UI for activity review, quick logging
   - Automerge sync client in Rust core
   - **Separate codebases:** Kotlin (Android), Swift (iOS)

3. **Sync:**
   - Same as Option 1 (local WiFi, relay server, Bluetooth)
   - Desktop ↔ Mobile sync is primary use case
   - Tauri backend handles sync in Rust (no JSON overhead for sync layer)

**Pros:**
- ✅ Tauri desktop app faster to develop than native (React UI, web dev familiar)
- ✅ True local-first sync (Automerge)
- ✅ Rust core shared across desktop + mobile
- ✅ Single desktop codebase for macOS/Windows/Linux
- ✅ Mature Tauri ecosystem (v2.x production-ready)

**Cons:**
- ❌ **JSON IPC overhead** for every frontend ↔ backend call (commands.rs)
- ❌ **Data duplication risk** - BeliefSet in Rust backend, cached in JS frontend
- ❌ Still need 2 separate mobile apps (Android Kotlin, iOS Swift)
- ❌ React state management complexity (syncing with Rust backend state)
- ❌ 3 separate UI codebases (React desktop, Kotlin Android, Swift iOS)
- ❌ Automerge integration complexity
- ❌ iOS dwelling point detection limited

**Estimated Effort:** 3-4 months for Tauri desktop, 6-9 months per mobile platform (total: 15-21 months)

---

### Option 3: Tauri Desktop + PWA Mobile + Limited Sync

**Technology Stack:**
- **Desktop:** Tauri (Rust + React)
- **Mobile:** Progressive Web App (PWA) with limited sensor access
- **Sync:** Manual export/import or simple cloud sync (optional)
- **Data:** SQLite (desktop), IndexedDB (PWA)

**How It Works:**

1. **Desktop (Tauri):** Full-featured authoring tool (as Option 2)

2. **Mobile (PWA):**
   - Web app installed via browser "Add to Home Screen"
   - **No dwelling point detection** (browser APIs insufficient)
   - Manual activity logging only (participant enters activities)
   - Curriculum lessons, visualization review
   - Limited background execution

3. **Sync:**
   - **Option 3a:** Manual export/import (JSON file transfer)
   - **Option 3b:** Simple cloud sync via optional relay server (not CRDT)
   - **Option 3c:** No sync (desktop and mobile are independent)

**Pros:**
- ✅ Fastest to develop (single web codebase)
- ✅ No app store approval process (mobile PWA)
- ✅ Rust core used in Tauri backend

**Cons:**
- ❌ **Fatal flaw:** No dwelling point detection (core value proposition)
- ❌ Manual activity logging only (high friction, low compliance)
- ❌ PWA limitations (notifications, background sync unreliable)
- ❌ Not truly cross-platform (desktop works, mobile crippled)

**Verdict:** Not viable for MVP. Could be post-beta for web-only tier.

---

### Option 4: Native Mobile + Tauri Desktop + SQLite Export/Import (No Auto-Sync)

**Technology Stack:**
- **Mobile:** Native Android (primary), iOS (deferred)
- **Desktop:** Tauri (Rust + React)
- **Sync:** Manual SQLite database export/import (participant-initiated)
- **Data:** SQLite on all platforms (same schema)

**How It Works:**

1. **Mobile (Native Android):**
   - Full dwelling point detection and activity logging
   - Rust core via UniFFI
   - Export SQLite database: `/storage/emulated/0/Download/noet_backup_20260315.db`

2. **Desktop (Tauri):**
   - Import SQLite database (participant transfers via USB, cloud storage, or email)
   - Full lattice editing, visualization, curriculum
   - Export updated database back to mobile

3. **Sync Workflow:**
   - Participant decides when to sync (weekly review workflow)
   - Mobile → Desktop: Export DB, transfer file, import on desktop
   - Desktop → Mobile: Export updated DB, transfer file, import on mobile
   - **Conflict resolution:** Last-write-wins (participant manually resolves)

**Pros:**
- ✅ Simple to implement (no CRDT, no sync protocol)
- ✅ True local-first (participant controls file transfers)
- ✅ SQLite format is portable, inspectable, backup-friendly
- ✅ Tauri desktop + native mobile (best of both worlds)
- ✅ No cloud dependency or relay server infrastructure

**Cons:**
- ❌ Manual sync is friction (participant must remember to transfer)
- ❌ Conflict resolution is manual (risky for concurrent edits)
- ❌ Poor UX for multi-device users (work phone + personal phone)
- ❌ No real-time sync (desktop visualizations stale until next import)

**Verdict:** Viable MVP approach. Simple, pragmatic, aligns with contemplative pace. Add auto-sync post-beta if participants demand it.

---

### Option 5: Native Mobile + Tauri Desktop + Syncthing Integration

**Technology Stack:**
- **Mobile:** Native Android
- **Desktop:** Tauri
- **Sync:** Syncthing (participant-controlled P2P file sync)
- **Data:** SQLite + write-ahead log (WAL)

**How It Works:**

1. **Syncthing Setup:**
   - Participant installs Syncthing on all devices
   - Configures sync folder: `~/Noet/data/`
   - Syncthing handles file-level P2P sync (no Noet code involved)

2. **Noet Integration:**
   - SQLite database stored in synced folder
   - Use WAL mode to minimize lock contention
   - Noet detects file changes (inotify/FSEvents) and reloads data
   - **Conflict handling:** SQLite WAL + participant-resolved conflicts

3. **UX:**
   - Participant sets up Syncthing once during onboarding
   - Automatic background sync (whenever devices are on same network)
   - Noet UI shows sync status (via Syncthing API)

**Pros:**
- ✅ Automatic sync (better UX than Option 4)
- ✅ Participant-controlled (self-hosted, no Noet servers)
- ✅ Syncthing is mature, battle-tested, privacy-focused
- ✅ Noet doesn't implement sync (leverages existing tool)
- ✅ Works offline (Syncthing queues changes)

**Cons:**
- ❌ Requires Syncthing installation (friction in onboarding)
- ❌ SQLite concurrent writes are tricky (WAL helps but not perfect)
- ❌ Conflict detection/resolution still needed (file conflicts possible)
- ❌ Mobile Syncthing app battery concerns
- ❌ Not a "just works" experience (power user solution)

**Verdict:** Interesting for post-MVP "advanced" tier. Too much setup for beta participants.

---

## Automerge Deep Dive

Given Options 1 and 2 both use Automerge, let's evaluate its production-readiness for Noet.

### Automerge 2.0 (2023+)

**Status:** Ink & Switch declares it production-ready as of 2023.

**Features:**
- CRDT-based conflict-free merging
- Rich data types (maps, lists, text, counters)
- Efficient sync protocol (only sends changes)
- Rust implementation (`automerge-rs`) with FFI bindings
- Peer-to-peer sync (no central server required)
- Optional relay servers for cross-network sync

**Integration with SQLite:**

Automerge is not a database—it's a sync layer. Two approaches:

#### Approach A: Automerge as Source of Truth

```rust
// Lattice stored as Automerge document
let mut doc = AutoCommit::new();
doc.put(ROOT, "dimensions", automerge::Value::Map(...))?;

// Sync via network
let sync_message = doc.sync();
send_to_peer(sync_message);

// Materialize to SQLite for querying
let dimensions = doc.get(ROOT, "dimensions")?;
db.execute("INSERT INTO dimensions ...")?;
```

**Pros:**
- Automerge handles all conflict resolution
- SQLite is derived view (can rebuild from Automerge)

**Cons:**
- Query performance (must materialize to SQLite for complex queries)
- Automerge storage overhead (full CRDT history)

#### Approach B: SQLite as Source of Truth + Automerge Sync Log

```rust
// Write to SQLite as usual
db.execute("INSERT INTO actions (id, name) VALUES (?, ?)", (id, name))?;

// Separately, record change in Automerge
let mut sync_doc = AutoCommit::new();
sync_doc.put(ROOT, "actions", vec![ChangeLog { op: Insert, id, name }])?;

// Sync Automerge log
send_sync_message(sync_doc.sync());

// On receive, apply changes to local SQLite
apply_changes_to_sqlite(received_changes)?;
```

**Pros:**
- SQLite remains fast query layer
- Automerge only tracks changes (smaller overhead)

**Cons:**
- Conflict resolution logic in app code (Automerge only syncs change log)
- More complex integration

**Recommendation:** Start with **Approach A** for lattice (low write frequency), use **Approach B** for activity logs (high write frequency, append-only).

---

### Automerge Rust Integration

**Library:** `automerge-rs` (https://github.com/automerge/automerge-rs)

**FFI Bindings:**
- `uniffi-rs` can wrap Automerge Rust types
- Expose to Kotlin (Android), Swift (iOS), WASM (web)

**Example Integration:**

```rust
// In rust_core/crates/sync/

use automerge::{AutoCommit, ROOT};
use uniffi;

#[uniffi::export]
pub struct SyncManager {
    doc: AutoCommit,
}

#[uniffi::export]
impl SyncManager {
    pub fn new() -> Self {
        Self { doc: AutoCommit::new() }
    }
    
    pub fn set_aspiration(&mut self, id: String, name: String) -> Result<(), SyncError> {
        self.doc.put(ROOT, id, name)?;
        Ok(())
    }
    
    pub fn sync_message(&mut self) -> Vec<u8> {
        self.doc.save()
    }
    
    pub fn apply_sync(&mut self, message: Vec<u8>) -> Result<(), SyncError> {
        self.doc.load(&message)?;
        Ok(())
    }
}
```

**Kotlin (Android):**

```kotlin
val syncManager = SyncManager()
syncManager.setAspiration("asp-001", "Be present")
val msg = syncManager.syncMessage()

// Send to peer via TCP/Bluetooth
sendToPeer(msg)

// Receive from peer
val receivedMsg = receiveFromPeer()
syncManager.applySync(receivedMsg)
```

---

### Peer Discovery Options

For local-first sync, devices must discover each other. Options:

#### 1. mDNS/Bonjour (Local Network)

**How:**
- Device advertises: `_noet._tcp.local` service
- Other devices discover via mDNS query
- Establish TCP connection

**Pros:**
- Zero-config on same WiFi
- Standard protocol (Bonjour on macOS/iOS, Android mDNS)

**Cons:**
- Only works on local network (can't sync phone → desktop at work)

#### 2. QR Code Pairing

**How:**
- Desktop displays QR code with: `noet://sync?peer_id=abc123&ip=192.168.1.5&port=8080`
- Mobile scans QR, initiates connection
- Exchange public keys for future encrypted sync

**Pros:**
- User-controlled (explicit trust)
- Works across networks (if relay server used)

**Cons:**
- Requires participant action (not automatic)

#### 3. Relay Server (Optional, Paid Tier)

**How:**
- Devices register with relay server: `relay.noet.app`
- Exchange Automerge messages via relay
- End-to-end encrypted (relay can't read data)

**Pros:**
- Works across networks (phone on cellular → desktop on home WiFi)
- Still privacy-preserving (relay is blind)

**Cons:**
- Requires Noet infrastructure (but no data storage)
- Potential revenue model ($5/month for hosted relay)

---

## Recommended Architecture

Based on constraints, performance requirements (no data duplication), and long-term maintainability:

### **Recommendation: Flutter + rinf (Option 2) for all platforms**

**Rationale:**

1. **No Data Duplication:**
   - Single source of truth: BeliefSet lives in Rust, Dart queries via FFI
   - No "silly translation" - direct Protobuf messages, no JSON IPC
   - compiler.rs and commands.rs exposed directly to Dart

2. **Single Codebase:**
   - One UI codebase for iOS, Android, macOS, Windows, Linux
   - Brandy can author content once, works everywhere
   - LLM-assisted development (Claude/Gemini excel at Flutter)

3. **Performance:**
   - rinf FFI faster than Tauri JSON IPC
   - d3.js via WebView acceptable for read-only visualizations
   - Native UI performance (60fps scrolling, animations)

4. **Development Speed:**
   - 4-6 months for all platforms vs. 15-21 months for Tauri + native mobile
   - Hot reload accelerates iteration
   - Single codebase = single test suite

5. **Existing Progress Leveraged:**
   - Android ForegroundService already built (Kotlin) → reuse via platform channel
   - Rust core (Milestones 1-5 complete) → expose via rinf, no rewrite
   - SQLite schema unchanged

**Trade-offs Accepted:**
- WebView overhead for d3.js (acceptable - visualizations are read-only, not interactive)
- rinf learning curve (newer than UniFFI, but better performance)
- Flutter bundle size ~20-30MB (acceptable for modern devices)

---

### **Implementation Phases**

#### **Phase 1: Flutter + rinf MVP (4-6 months)**

**Weeks 1-8: Desktop App Foundation**
- Set up Flutter desktop project (macOS/Windows/Linux)
- Integrate rinf with rust_core
- Expose commands.rs operations (GetStates, SetContent, GetFocus, etc.)
- Build curriculum reader UI (Lessons 1-7)
- Implement lattice editor (TOML editing with live preview)
- BYOLLM integration (API key management, Socratic prompts)
- **No manual sync yet** - desktop reads local SQLite directly

**Weeks 9-16: Mobile App with Dwelling Point Detection**
- Flutter mobile project (Android + iOS)
- Platform channel for ForegroundService (reuse existing Kotlin code)
- Platform channel for CoreLocation (new Swift code for iOS)
- Integrate rinf with same rust_core (100% code reuse)
- Mobile curriculum UI (same Dart code as desktop)
- Activity log review screens
- **Export/Import SQLite** - manual sync for MVP

**Weeks 17-24: Visualizations & Beta Prep**
- flutter_inappwebview integration
- d3.js constellation view (Rust generates HTML/JS, WebView renders)
- d3.js temporal score
- Glyph rendering (Canvas in Flutter OR d3.js in WebView)
- Performance testing on physical devices
- Beta onboarding flow

**Deliverables:**
- Single Flutter codebase running on 5 platforms (Android, iOS, macOS, Windows, Linux)
- No JSON IPC overhead (rinf FFI)
- No data duplication (Rust owns BeliefSet)
- Manual SQLite export/import for sync (acceptable for MVP)

---

#### **Phase 2: Automerge Auto-Sync (6-12 months post-beta)**

**Only if beta participants demand auto-sync:**

1. Add `rust_core/crates/sync/` with Automerge
2. Expose Automerge operations via rinf to Flutter
3. Implement mDNS discovery (Dart package: multicast_dns)
4. Build sync UI in Flutter (status indicators, conflict resolution)
5. Optional: Deploy relay server for paid tier

**Sync Workflow:**
- Same as Option 2b (auto-discover, background sync, conflict-free)
- All implemented in Dart + Rust, no platform-specific code

---

### **Alternative: MVP with Tauri Desktop + Native Android (Fallback)**

**If Flutter + rinf prototyping reveals unexpected blockers:**

**Option 4: Native Android + Tauri Desktop + Manual SQLite Export/Import**

**Rationale:**
- Simplest to implement (no auto-sync, no CRDT)
- Leverages existing Android app (Milestones 1-5 complete)
- Tauri desktop faster than native desktop
- Aligns with contemplative pace (weekly manual sync)

**Implementation:**
1. Keep existing Native Android app with dwelling point detection
2. Build Tauri desktop app for authoring and visualization (Weeks 1-10)
3. Manual SQLite export/import (participant transfers files)

**Limitations:**
- JSON IPC overhead in Tauri (every frontend ↔ backend call)
- Data duplication risk (JS caches lattice state)
- 2 separate UI codebases (React desktop, Kotlin Android)
- iOS deferred indefinitely (separate codebase)

**Only choose this if:**
- rinf proves too immature (prototype fails)
- WebView performance is unacceptable for visualizations
- Need to ship desktop app in <8 weeks (no time for Flutter learning curve)

---

## iOS Considerations

**Dwelling Point Detection on iOS:**

iOS severely limits background sensor access:
- WiFi RSSI data deprecated (iOS 13+)
- BLE scanning in background possible but battery-constrained
- GPS available via "Always Allow" permission (user hostile)
- Significant location changes API (100-500m accuracy, not fine-grained)

**Implication:** iOS cannot achieve same dwelling point detection quality as Android.

**Options:**

1. **Deferred iOS (Recommended for MVP):**
   - Launch with Android only
   - iOS participants use desktop app + manual logging
   - Build iOS app post-beta if demand warrants

2. **iOS Lite (Post-Beta):**
   - Use significant location changes API only
   - Lower dwelling point resolution (neighborhood-level, not room-level)
   - Supplement with manual logging
   - Clearly communicate limitations to iOS users

3. **iOS with External Hardware (Speculative):**
   - Participant carries Bluetooth beacon
   - iOS app tracks beacon proximity (BLE allowed in background)
   - Dwelling points = beacon locations
   - High friction (requires hardware purchase)

**Recommendation:** Defer iOS until Android beta validates product-market fit. If iOS demand is strong, explore Option 2 (iOS Lite).

---

## Open Questions

### 1. Automerge Storage Overhead

**Question:** How much storage does Automerge CRDT history consume for a 2-year participant lattice?

**Research Needed:**
- Benchmark: Create lattice with 500 nodes, 1000 edits over simulated 2 years
- Measure Automerge document size
- Compare to raw SQLite size

**Mitigation:** Automerge supports history pruning (discard old changes). May be acceptable to prune history older than 6 months.

---

### 2. Conflict Resolution UX

**Question:** When concurrent edits conflict, how does participant resolve?

**Options:**
- **Automerge auto-merge:** Trust CRDT (usually correct, but may surprise participant)
- **Diff UI:** Show changes from each device, let participant choose
- **Warn on export:** "You edited on phone since last import. Overwrite or merge?"

**Research Needed:** User testing with beta participants. Observe conflict frequency and resolution strategies.

---

### 3. d4 Package Viability (Dart port of d3)

**Question:** Can we fork and maintain d4 with LLM assistance, or is it too abandoned?

**Package Analysis:**
- **URL:** https://pub.dev/packages/d4
- **Last Update:** 2021 (3+ years ago)
- **Features:** Selections, scales, shapes, axes, transitions, force simulation
- **Missing:** Some d3.js modules (geo, chord, sankey)
- **Status:** Abandoned by original maintainer

**LLM-Assisted Maintenance Strategy:**

**Advantages:**
1. **Dart is LLM-friendly:** Claude/Gemini excellent at Dart code generation
2. **d3.js well-documented:** Can port features from d3.js v7 with LLM help
3. **Noet only needs subset:** Force-directed layout, scales, paths (not all d3.js)
4. **Single maintainer (Andrew):** No need for community governance

**Maintenance Plan:**
1. **Fork d4 to Noet organization**
2. **Audit what Noet needs:**
   - Force simulation (for constellation view)
   - Scales (color, linear, time)
   - Path generators (for edges)
   - Shape generators (for glyphs)
3. **Test existing features** with constellation prototype
4. **Port missing features** from d3.js v7 as needed (with LLM assistance)
5. **Document Noet-specific usage** patterns

**Risk Assessment:**
- **Low risk:** Noet uses narrow subset of d3.js (force layout + scales)
- **Mitigated:** If d4 fundamentally broken, fallback to WebView (Option B)
- **Opportunity:** Could contribute back to Dart ecosystem if successful

**Estimated Effort:**
- Fork and audit: 1 day
- Fix/extend for constellation view: 3-7 days
- Ongoing maintenance: <1 day/month (as bugs found)

**Recommendation:** Worth attempting in Week 2 prototype. If force simulation works with minor fixes, d4 is viable. If requires >1 week of debugging, fallback to WebView.

---

### 4. Git Integration for Intention Lattice

**Requirement:** Flutter app must perform Git operations (clone, pull, push, commit) with SSH key authentication.

**Flutter Git Libraries:**

1. **git_bindings** (https://pub.dev/packages/git_bindings)
   - FFI wrapper around libgit2 (native Git library)
   - Supports clone, pull, push, commit, branch, merge
   - SSH key authentication via libssh2
   - **Status:** Maintained, production-ready
   - **Platform support:** Android, iOS, macOS, Windows, Linux

2. **Alternative: Shell out to system Git**
   - Use `Process.run('git', ['pull'])` via Dart `dart:io`
   - Requires Git installed on system (not guaranteed on mobile)
   - Less reliable, but simpler

**Recommendation:** Use `git_bindings` for cross-platform Git integration.

**Git Workflow in Noet:**

1. **Initial Setup (Onboarding):**
   - Participant creates GitHub/GitLab account (if don't have)
   - Creates private repo: `{username}/noet-lattice`
   - Or forks public template: `noet-community/starter-lattice`
   - App clones repo to local storage: `~/Noet/lattices/{repo_name}/`
   - SSH key generated and added to GitHub/GitLab account

2. **Daily Editing:**
   - Participant edits lattice files (TOML) in app
   - Local changes tracked by Git (working directory)
   - **Auto-commit** on file save (optional): `git commit -m "Update: {file_name}"`
   - **Manual commit** via "Save Checkpoint" button

3. **Sync (Pull/Push):**
   - **Pull on launch:** `git pull origin main` to get latest changes
   - **Push on demand:** "Sync Lattice" button → `git push origin main`
   - **Conflict resolution:** If merge conflict, show diff UI and let participant resolve

4. **Community Sharing:**
   - Participant makes repo public (GitHub setting)
   - Others can clone: "Import Lattice from URL"
   - Fork/contribute workflow: standard Git pull requests
   - Noet community builds library of shared lattices (action libraries, starter templates)

**Git UI Components Needed:**

- **Clone dialog:** Enter GitHub/GitLab URL, authenticate
- **Sync button:** Pull + Push with status indicator
- **Conflict resolution UI:** Show diff, allow manual merge or "take theirs/mine"
- **Commit history view:** Browse past versions (git log)
- **Branch switcher:** Switch between branches (post-MVP)
- **SSH key manager:** Generate, view, copy public key

**SSH Key Management:**

- Generate ED25519 key pair on device
- Store private key securely (Keychain on iOS, KeyStore on Android)
- Display public key for user to add to GitHub/GitLab
- Optionally: Use GitHub's device authorization flow (OAuth)

**Benefits of Git-based Lattice Sync:**

1. **Backup:** Remote repository is automatic backup
2. **Version control:** See lattice evolution over time (`git log`)
3. **Community:** Participants share starter lattices, action libraries
4. **Collaboration:** Fork/contribute to community templates
5. **Offline-first:** Commit locally, push when online
6. **Portable:** Lattice is just a directory of TOML files (platform-independent)
7. **Developer-friendly:** Participants who know Git get full power

**Limitations:**

- **Learning curve:** Non-technical participants need Git primer
- **Conflict resolution:** Merge conflicts require understanding
- **SSH setup:** Adding public key to GitHub/GitLab is friction

**Mitigation:**
- **Guided onboarding:** Step-by-step wizard with screenshots
- **Simplified UI:** "Sync" button hides `git pull && git push`
- **Auto-conflict resolution:** If possible, auto-merge non-overlapping changes
- **Alternative:** Offer "Noet Cloud" tier with simplified sync (no Git exposed)

---

### 5. Multi-Device Activity Logs

**Question:** Participant has work phone + personal phone. How to merge activity logs?

**Challenge:** Two phones generating dwelling point visits simultaneously (different locations).

**Options:**
- **Primary device only:** Participant designates one phone for tracking
- **Multi-device merge:** Automerge activity logs from both phones (complex: same timestamps, different locations)

**Recommendation:** MVP supports single device only. Post-beta, add multi-device if demanded.

---

### 4. Backup and Restore

**Question:** Participant loses phone. How to restore data?

**Options:**
- **SQLite export to cloud:** Participant manually backs up to Dropbox/iCloud/Google Drive
- **Automerge relay server:** Acts as backup (always has latest sync state)
- **Desktop as backup:** As long as participant synced recently, desktop has copy

**Recommendation:** Document SQLite export workflow. Encourage weekly backups. Post-beta, add encrypted cloud backup as paid feature.

---

## Decision Framework

Use this framework to validate Flutter + rinf recommendation:

### Questions to Answer:

1. **Is rinf FFI stable enough for production?**
   - **Test:** Build minimal Flutter app with rinf calling commands.rs
   - **Success criteria:** GetStates, SetContent operations work without crashes
   - **Timeline:** 1 week prototype
   - **Risk:** If rinf is too immature → fallback to Tauri (Option 4)

2. **Is WebView performance acceptable for d3.js visualizations?**
   - **Test:** Render constellation view with 100+ nodes in flutter_inappwebview
   - **Success criteria:** 60fps pan/zoom, <2s initial render
   - **Timeline:** 1 week prototype
   - **Risk:** If WebView too slow → consider Flutter Canvas rendering (harder)

3. **Can we avoid data duplication between Rust and Dart?**
   - **Test:** Query BeliefSet via rinf, display in Flutter UI, measure memory
   - **Success criteria:** No duplicate lattice storage in Dart
   - **Timeline:** 1 week prototype
   - **Risk:** If duplication necessary → rinf not solving core problem

4. **Is Flutter + rinf faster than Tauri + separate mobile?**
   - **Estimate:** 4-6 months (Flutter all platforms) vs. 15-21 months (Tauri + native mobile)
   - **Success criteria:** Flutter delivers 3x faster to all platforms
   - **Validation:** Confirm with prototype velocity (features/week)

5. **Does Brandy prefer Flutter or React for UI development?**
   - **Test:** Build curriculum lesson reader in both (1 day each)
   - **Success criteria:** Brandy can iterate on content independently
   - **Consider:** LLM assistance quality (Claude/Gemini with Flutter vs React)

---

## Next Steps

### Phase 1: Validate Flutter + rinf (1-2 weeks)

**Week 1: rinf Integration Prototype**
1. Create minimal Flutter project (desktop + Android)
2. Add rinf dependency, configure Protobuf messages
3. Expose commands.rs operations (GetStates, SetContent) via rinf
4. Build simple UI: Query lattice, display results
5. **Success criteria:**
   - No crashes
   - No data duplication (Rust owns BeliefSet, Dart receives query results)
   - FFI call latency <10ms for typical query
   - Memory usage comparable to native Android app

**Week 2: Visualization Strategy Prototype**

Test all three approaches with constellation view (100+ nodes):

**2a. d4 (Dart port of d3) Test:**
1. Fork d4 package (https://pub.dev/packages/d4)
2. Implement force-directed layout for constellation
3. Render to Flutter Canvas widget
4. Test with LLM assistance to fix/extend abandoned features
5. **Success criteria:**
   - Force simulation works for 100+ nodes
   - Pan/zoom via GestureDetector works
   - Can extend/maintain with LLM help
   - Performance: 60fps, <2s render

**2b. WebView Test (if d4 fails):**
1. Integrate flutter_inappwebview
2. Rust generates HTML/JS for d3.js constellation
3. Render in WebView, test interactions
4. **Success criteria:**
   - 60fps pan/zoom
   - <2s render
   - No janky input lag

**2c. CustomPaint Test (if both fail):**
1. Implement simple force-directed layout in Dart
2. Render with CustomPaint widget
3. Estimate effort to complete (likely weeks)

**Deliverable:** Technical report with visualization strategy recommendation + Flutter architecture decision

---

### Phase 2: Architecture Decision Meeting (1 day)

**Agenda:**
1. Review rinf prototype results
2. Review WebView performance data
3. Compare Flutter vs Tauri effort estimates (4-6mo vs 15-21mo)
4. Decide: Flutter + rinf OR Tauri + native mobile
5. Update slc_roadmap.md with chosen architecture

**Attendees:** Andrew + Brandy

**Outcomes:**
- Architecture finalized
- Timeline adjusted
- Roadmap updated with implementation phases

---

### Phase 3: If Flutter Selected → Alpha Development (Weeks 1-8)

**Weeks 1-4: Desktop App MVP**
- Flutter desktop project setup
- rinf integration with full rust_core
- Curriculum reader UI (7 lessons)
- Lattice editor (TOML with live preview)
- BYOLLM setup wizard (OpenAI/Anthropic/Ollama)

**Weeks 5-8: Brandy Alpha Testing**
- Brandy uses desktop app daily
- Documents friction points
- Tests Socratic prompts with BYOLLM
- Validates curriculum content rendering
- Provides UX feedback for iteration

**Milestone:** Desktop app ready for Brandy's daily use (content authoring workflow)

---

### Phase 4: If Tauri Selected → Alternative Path

**Fallback to Option 4: Native Android + Tauri Desktop**

**Weeks 1-4: Tauri Desktop MVP**
- Tauri project setup (React + Rust backend)
- Expose commands.rs via Tauri invoke (JSON IPC)
- React UI for curriculum + lattice editor
- d3.js visualizations (no WebView, native React integration)

**Weeks 5-8: Brandy Alpha Testing**
- Same workflow as Flutter path
- Accept JSON IPC overhead (mitigate by minimizing calls)
- Accept data duplication risk (monitor memory usage)

**Limitation:** iOS deferred indefinitely (no shared codebase)

---

## Conclusion

Noet's cross-platform architecture must balance **native platform access** (dwelling point detection), **code reuse** (Rust core), **local-first sync** (participant data sovereignty), and **no data duplication** (single source of truth).

**Primary Recommendation: Flutter + rinf (Option 2)**

- **Single codebase** for iOS, Android, macOS, Windows, Linux (4-6 months to all platforms)
- **No JSON IPC overhead** - rinf FFI with Protobuf messages
- **No data duplication** - Rust owns BeliefSet, Dart queries via FFI
- **compiler.rs and commands.rs** exposed directly to Dart
- **Existing progress leveraged** - Android ForegroundService reused via platform channel, Rust core (Milestones 1-5) unchanged
- **LLM-assisted development** - Claude/Gemini excellent with Flutter

**Fallback: Tauri + Native Android (Option 4)**

- If rinf proves too immature (1-2 week prototype fails)
- Accept JSON IPC overhead and data duplication risk
- 2 separate UI codebases (React desktop, Kotlin Android)
- iOS deferred indefinitely

**For Sync:** 
- **Intention Lattice:** Git-based (push/pull to GitHub/GitLab) from day 1
  - Enables backup, version control, community sharing
  - Use git_bindings Flutter package for cross-platform Git operations
- **Activity Logs:** Start with no cross-device sync (MVP). Add Automerge post-beta if demanded

**For iOS Dwelling Point Detection:** Build with CoreLocation limitations (degraded accuracy) or defer until Android beta validates PMF.

**Key Insight:** Flutter + rinf solves the core constraint (no data duplication, no JSON translation) while delivering to all platforms 3x faster than alternatives. The 1-2 week prototype is critical to validate this recommendation before committing.

---

## References

- Ink & Switch: Automerge 2.0 announcement (https://automerge.org/blog/automerge-2/)
- Automerge Rust: https://github.com/automerge/automerge-rs
- Local-First Software principles: https://www.inkandswitch.com/local-first/
- iOS Background Location: https://developer.apple.com/documentation/corelocation/getting_the_user_s_location
- Android ForegroundService: https://developer.android.com/develop/background-work/services/foreground-services
- Syncthing: https://syncthing.net/
