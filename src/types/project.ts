/** 项目工程目录（绑定一个 Claude provider） */
export interface Project {
  id: string;
  name: string;
  path: string;
  description?: string;
  /** 绑定的 Claude provider id（引用全局 claude provider 池） */
  claudeProviderId?: string;
  /** 创建时间（Unix 毫秒） */
  createdAt: number;
  /** 最后更新时间（Unix 毫秒） */
  updatedAt: number;
  /** 上次成功写入项目根 .claude/settings.json 的时间 */
  lastWrittenAt?: number;
  /** 软删除时间（undefined = 未删除） */
  deletedAt?: number;
  sortIndex?: number;
  icon?: string;
  iconColor?: string;
}

export interface CreateProjectRequest {
  name: string;
  path: string;
  description?: string;
  claudeProviderId?: string;
  icon?: string;
  iconColor?: string;
}

/** 所有字段可选，undefined 表示不改 */
export interface UpdateProjectRequest {
  name?: string;
  path?: string;
  description?: string;
  icon?: string;
  iconColor?: string;
}

export interface ProjectPathValidation {
  exists: boolean;
  isDirectory: boolean;
  writable: boolean;
  parentExists: boolean;
}
