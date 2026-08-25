/**
 * Tool schemas advertised to the model, plus the gating tool sets. The
 * orchestrator is the only place that knows how to execute these; providers
 * just forward the definitions.
 */
export const WORKSPACE_TOOLS = [
  {
    type: "function" as const,
    function: {
      name: "write_file",
      description: "Create or overwrite a workspace file. Prefer edit_file for small changes.",
      parameters: {
        type: "object",
        properties: {
          path: {
            type: "string",
            description: "Workspace-relative file path."
          },
          content: {
            type: "string",
            description: "Exact file content."
          }
        },
        required: ["path", "content"]
      }
    }
  },
  {
    type: "function" as const,
    function: {
      name: "edit_file",
      description: "Replace exact text in an existing file. old_string must match exactly.",
      parameters: {
        type: "object",
        properties: {
          path: {
            type: "string",
            description: "Workspace-relative file path."
          },
          old_string: {
            type: "string",
            description: "Exact text to replace. Must be unique unless replace_all is true."
          },
          new_string: {
            type: "string",
            description: "Replacement text."
          },
          replace_all: {
            type: "boolean",
            description: "Replace every occurrence. Defaults to false."
          }
        },
        required: ["path", "old_string", "new_string"]
      }
    }
  },
  {
    type: "function" as const,
    function: {
      name: "delete_file",
      description: "Delete a workspace file.",
      parameters: {
        type: "object",
        properties: {
          path: {
            type: "string",
            description: "Workspace-relative file path."
          }
        },
        required: ["path"]
      }
    }
  },
  {
    type: "function" as const,
    function: {
      name: "read_file",
      description: "Read a workspace file. Large files are returned in windows of up to 2000 lines; the result then includes total_lines and a notice telling you which offset to request next.",
      parameters: {
        type: "object",
        properties: {
          path: {
            type: "string",
            description: "Workspace-relative file path."
          },
          offset: {
            type: "number",
            description: "1-based line number to start reading from. Use together with limit to page through large files."
          },
          limit: {
            type: "number",
            description: "Maximum number of lines to return (default and cap: 2000)."
          }
        },
        required: ["path"]
      }
    }
  },
  {
    type: "function" as const,
    function: {
      name: "list_directory",
      description: "List files and folders in a workspace directory.",
      parameters: {
        type: "object",
        properties: {
          path: {
            type: "string",
            description: "Workspace-relative directory path; use '' or '.' for root."
          }
        },
        required: ["path"]
      }
    }
  },
  {
    type: "function" as const,
    function: {
      name: "create_directory",
      description: "Create a workspace directory, including parents.",
      parameters: {
        type: "object",
        properties: {
          path: {
            type: "string",
            description: "Workspace-relative directory path."
          }
        },
        required: ["path"]
      }
    }
  },
  {
    type: "function" as const,
    function: {
      name: "move_file",
      description: "Move or rename a workspace file or directory.",
      parameters: {
        type: "object",
        properties: {
          source_path: {
            type: "string",
            description: "Current workspace-relative path."
          },
          destination_path: {
            type: "string",
            description: "New workspace-relative path."
          }
        },
        required: ["source_path", "destination_path"]
      }
    }
  },
  {
    type: "function" as const,
    function: {
      name: "search_files",
      description: "Search workspace file names and contents. Skips node_modules and .git.",
      parameters: {
        type: "object",
        properties: {
          query: {
            type: "string",
            description: "Text to search for."
          },
          path: {
            type: "string",
            description: "Optional workspace-relative subdirectory."
          }
        },
        required: ["query"]
      }
    }
  },
  {
    type: "function" as const,
    function: {
      name: "search_symbols",
      description: "Search functions, classes, interfaces, and types across the codebase by name, kind, or signature.",
      parameters: {
        type: "object",
        properties: {
          query: {
            type: "string",
            description: "Symbol name or substring to find (e.g. 'executeCommand', 'AuthService', 'User')."
          },
          kind: {
            type: "string",
            description: "Optional symbol kind: 'function', 'class', 'interface', 'type', 'variable'.",
            enum: ["function", "class", "interface", "type", "variable"]
          }
        },
        required: ["query"]
      }
    }
  },
  {
    type: "function" as const,
    function: {
      name: "run_command",
      description: "Run a shell command in the workspace. Requires user approval.",
      parameters: {
        type: "object",
        properties: {
          command: {
            type: "string",
            description: "Command to execute."
          },
          network_domains: {
            type: "array",
            items: { type: "string" },
            description: "Exact outbound host names required by this command. Omit for a network-isolated command. New domains always require approval."
          },
          timeout_ms: {
            type: "integer",
            minimum: 1000,
            maximum: 900000,
            description: "Maximum runtime in milliseconds. Defaults to 600000 (10 minutes); use a larger value for long builds or test suites."
          }
        },
        required: ["command"]
      }
    }
  },
  {
    type: "function" as const,
    function: {
      name: "search_web",
      description: "Search the web and return top text results with title, URL, and snippet. Requires user approval.",
      parameters: {
        type: "object",
        properties: {
          query: {
            type: "string",
            description: "Web search query."
          },
          max_results: {
            type: "number",
            description: "Maximum number of results to return, from 1 to 10. Defaults to 5."
          }
        },
        required: ["query"]
      }
    }
  },
  {
    type: "function" as const,
    function: {
      name: "fetch_url",
      description: "Fetch an http(s) URL as text. Requires user approval.",
      parameters: {
        type: "object",
        properties: {
          url: {
            type: "string",
            description: "Absolute http(s) URL."
          }
        },
        required: ["url"]
      }
    }
  }
];

/**
 * Plan-mode-only tool. Lets the model create and maintain a structured,
 * chat-specific plan (title + ordered steps with status). The orchestrator
 * persists it to SQLite and pushes it to the plan panel; it never touches the
 * filesystem, so it runs without a permission prompt.
 */
export const UPDATE_PLAN_TOOL = {
  type: "function" as const,
  function: {
    name: "update_plan",
    description: "Record or revise the stable plan-panel document. Put detailed prose in body and steps in tasks; do not use it for progress updates.",
    parameters: {
      type: "object",
      properties: {
        title: {
          type: "string",
          description: "Short plan title."
        },
        body: {
          type: "string",
          description: "Full markdown plan shown in the panel."
        },
        tasks: {
          type: "array",
          description: "Ordered plan steps with status.",
          items: {
            type: "object",
            properties: {
              text: { type: "string", description: "Step text." },
              status: {
                type: "string",
                enum: ["pending", "in_progress", "completed"],
                description: "Step status."
              }
            },
            required: ["text", "status"]
          }
        },
        start_new: {
          type: "boolean",
          description: "True to replace a finished plan with a new effort."
        }
      },
      required: ["title", "tasks"]
    }
  }
};

/**
 * Lets the model name the chat session. Available in every mode; performs no
 * filesystem/network I/O (it only renames the run), so it runs silently without
 * a permission prompt. The orchestrator persists the title and broadcasts it.
 */
export const SET_TITLE_TOOL = {
  type: "function" as const,
  function: {
    name: "set_chat_title",
    description: "Set this chat's short title once intent is clear.",
    parameters: {
      type: "object",
      properties: {
        title: {
          type: "string",
          description: "Short title, 3-6 words, no quotes."
        }
      },
      required: ["title"]
    }
  }
};

/**
 * Lets the main agent ask the user one or more multiple-choice questions when it
 * needs a decision only the user can make. Performs no filesystem/network I/O —
 * the orchestrator pauses the run, surfaces the questions to the UI, and feeds
 * the chosen answers back as the tool result. Available to the main agent in
 * every mode; sub-agents never get it.
 */
export const ASK_QUESTION_TOOL = {
  type: "function" as const,
  function: {
    name: "ask_user_question",
    description: "Ask 1-4 multiple-choice questions only for decisions you cannot resolve from context or sensible defaults.",
    parameters: {
      type: "object",
      properties: {
        questions: {
          type: "array",
          description: "Questions to ask.",
          items: {
            type: "object",
            properties: {
              question: {
                type: "string",
                description: "Clear question text."
              },
              header: {
                type: "string",
                description: "Short label, max 12 chars."
              },
              multiSelect: {
                type: "boolean",
                description: "Allow multiple selections."
              },
              options: {
                type: "array",
                description: "Distinct choices.",
                items: {
                  type: "object",
                  properties: {
                    label: { type: "string", description: "Choice label." },
                    description: { type: "string", description: "Optional one-line explanation." }
                  },
                  required: ["label"]
                }
              }
            },
            required: ["question", "header", "options"]
          }
        }
      },
      required: ["questions"]
    }
  }
};

/**
 * Lets the main agent persist a durable fact across sessions. Performs no
 * filesystem/network I/O (it only writes a row to the memory table), so it is
 * not in DANGEROUS_TOOLS and runs silently without a permission prompt. Memories
 * are injected back into future runs' system prompt. Available to the main agent
 * in every mode; sub-agents never get it.
 */
export const REMEMBER_TOOL = {
  type: "function" as const,
  function: {
    name: "remember",
    description: "Save a durable preference, feedback, project fact, or reference in ENGLISH. Do not save transient details, secrets, or facts already in code/config.",
    parameters: {
      type: "object",
      properties: {
        scope: {
          type: "string",
          enum: ["global", "project"],
          description: "global for user-wide facts; project for this codebase."
        },
        category: {
          type: "string",
          enum: ["user", "feedback", "project", "reference"],
          description: "Memory category."
        },
        content: {
          type: "string",
          description: "Fact to remember, 1-3 concise English sentences."
        },
        update_id: {
          type: "number",
          description: "Existing memory id to update."
        }
      },
      required: ["scope", "category", "content"]
    }
  }
};

/**
 * Loads a skill's full SKILL.md body by name when the task matches an entry in
 * AVAILABLE SKILLS. Silent local disk read (allowlisted skill roots only); no
 * permission prompt. Main agent only; sub-agents never get it.
 */
export const LOAD_SKILL_TOOL = {
  type: "function" as const,
  function: {
    name: "load_skill",
    description: "Load a skill's full instructions by name when the task matches AVAILABLE SKILLS. Call before following a specialized workflow.",
    parameters: {
      type: "object",
      properties: {
        name: {
          type: "string",
          description: "Skill name from AVAILABLE SKILLS."
        }
      },
      required: ["name"]
    }
  }
};

/**
 * Architect-only tool (dual-model runs). Lets the architect model delegate one
 * or more self-contained coding tasks to coder sub-agents running the configured
 * coder model. Performs no direct filesystem/network I/O itself — the orchestrator
 * runs each sub-agent loop — so it is not in DANGEROUS_TOOLS and runs without a
 * permission prompt (the sub-agents' own tool calls are still gated normally).
 */
export const DELEGATE_TASKS_TOOL = {
  type: "function" as const,
  function: {
    name: "delegate_tasks",
    description: "Delegate 1-3 self-contained coding tasks to coder sub-agents. Use parallel only for disjoint files.",
    parameters: {
      type: "object",
      properties: {
        tasks: {
          type: "array",
          description: "Coding tasks to delegate.",
          items: {
            type: "object",
            properties: {
              title: { type: "string", description: "Short task title." },
              instructions: { type: "string", description: "Self-contained instructions (sub-agent sees only this). Keep short: describe the change and cite file paths; do NOT paste file contents — the coder reads files itself." },
              files: {
                type: "array",
                description: "Expected touched files.",
                items: { type: "string" }
              },
              verify: {
                type: "boolean",
                description: "Read-only verification task: the sub-agent may only read/list/search and must return a SHORT verdict on whether earlier changes are correct."
              }
            },
            required: ["title", "instructions"]
          }
        },
        parallel: {
          type: "boolean",
          description: "Run concurrently only when tasks cannot conflict."
        }
      },
      required: ["tasks"]
    }
  }
};

/**
 * Lightweight tier tool. Lets the architect delegate tiny, mechanical tasks
 * (locating files/symbols, summarizing a file, simple renames/moves) to a cheap
 * "utility" sub-agent running the configured utility model in this same workspace.
 * The sub-agent only gets read/list/search + move_file. It cannot run commands,
 * delete, edit, write, create, or delegate. It returns a SHORT distilled answer,
 * keeping the architect's context lean. Like delegate_tasks it performs no direct
 * I/O itself (the orchestrator runs the sub-agent loop), so it is not in
 * DANGEROUS_TOOLS.
 */
export const DELEGATE_UTILITY_TOOL = {
  type: "function" as const,
  function: {
    name: "delegate_to_utility",
    description: "Delegate 1-3 tiny lookup/summary/move tasks to utility sub-agents. No shell/delete/write/edit.",
    parameters: {
      type: "object",
      properties: {
        tasks: {
          type: "array",
          description: "Tiny utility tasks.",
          items: {
            type: "object",
            properties: {
              title: { type: "string", description: "Short task title." },
              instructions: { type: "string", description: "Self-contained instructions and the short answer wanted. Cite file paths; do NOT paste file contents — utility reads files itself." }
            },
            required: ["title", "instructions"]
          }
        },
        parallel: {
          type: "boolean",
          description: "Run concurrently only for independent tasks."
        }
      },
      required: ["tasks"]
    }
  }
};

/** Tools that must always be gated behind an explicit permission prompt. */
export const DANGEROUS_TOOLS = new Set(["run_command", "search_web", "fetch_url"]);

/**
 * Read-only tools — they only inspect the workspace, never mutate it or run
 * commands. These are the ONLY workspace tools allowed in plan mode (which is a
 * planning-only, no-changes mode). Everything else is blocked there.
 */
export const READONLY_TOOLS = new Set(["read_file", "list_directory", "search_files"]);

/**
 * Modifying tools — tools that create, modify, move, or delete files or directories,
 * or run shell commands. These are blocked in non-build modes.
 */
export const MODIFYING_TOOLS = new Set(["write_file", "edit_file", "delete_file", "create_directory", "move_file", "run_command"]);

/**
 * Tools available to a utility sub-agent: read-only inspection plus move_file
 * (rename/move). No write/edit/delete/create — utility work is lookups + safe
 * renames only.
 */
export const UTILITY_TOOL_NAMES = new Set([...READONLY_TOOLS, "move_file"]);
export const UTILITY_TOOLS = WORKSPACE_TOOLS.filter(t => UTILITY_TOOL_NAMES.has(t.function.name));
