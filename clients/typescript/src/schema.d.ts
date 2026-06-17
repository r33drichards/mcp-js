

export interface paths {
    "/api/cli": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        
        get: operations["cli_index_handler"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/cli/{platform}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        
        get: operations["cli_download_handler"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/exec": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        
        post: operations["exec_handler"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/executions": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        
        get: operations["list_executions_handler"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/executions/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        
        get: operations["get_execution_handler"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/executions/{id}/cancel": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        
        post: operations["cancel_execution_handler"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/executions/{id}/output": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        
        get: operations["get_execution_output_handler"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/version": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        
        get: operations["version_handler"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
}
export type webhooks = Record<string, never>;
export interface components {
    schemas: {
        
        ApiError: {
            error: string;
        };
        
        CancelResult: {
            error?: string | null;
            ok: boolean;
        };
        
        CliAsset: {
            
            available: boolean;
            
            platform: string;
            
            url: string;
        };
        
        CliIndex: {
            
            assets: components["schemas"]["CliAsset"][];
            
            version: string;
        };
        
        ExecAccepted: {
            
            execution_id: string;
        };
        
        ExecRequest: {
            
            code: string;
            
            execution_timeout_secs?: number | null;
            
            heap?: string | null;
            
            heap_memory_max_mb?: number | null;
            
            session?: string | null;
            
            tags?: {
                [key: string]: string;
            } | null;
        };
        
        ExecutionInfo: {
            
            completed_at?: string | null;
            
            error?: string | null;
            execution_id: string;
            
            heap?: string | null;
            
            result?: string | null;
            
            started_at: string;
            
            status: string;
        };
        
        ExecutionList: {
            executions: unknown[];
        };
        
        ExecutionOutput: {
            
            data: string;
            
            end_byte: number;
            
            end_line: number;
            execution_id: string;
            
            has_more: boolean;
            
            next_byte_offset: number;
            
            next_line_offset: number;
            
            start_byte: number;
            
            start_line: number;
            
            status: string;
            
            total_bytes: number;
            
            total_lines: number;
        };
        
        ExecutionSummary: {
            completed_at?: string | null;
            execution_id: string;
            started_at: string;
            status: string;
        };
        
        OutputQuery: {
            
            byte_limit?: number | null;
            
            byte_offset?: number | null;
            
            line_limit?: number | null;
            
            line_offset?: number | null;
        };
    };
    responses: never;
    parameters: never;
    requestBodies: never;
    headers: never;
    pathItems: never;
}
export type $defs = Record<string, never>;
export interface operations {
    cli_index_handler: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["CliIndex"];
                };
            };
        };
    };
    cli_download_handler: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                
                platform: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    exec_handler: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ExecRequest"];
            };
        };
        responses: {
            
            202: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ExecAccepted"];
                };
            };
            
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            
            415: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            
            500: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    list_executions_handler: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ExecutionList"];
                };
            };
            
            500: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    get_execution_handler: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ExecutionInfo"];
                };
            };
            
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    cancel_execution_handler: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["CancelResult"];
                };
            };
            
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["CancelResult"];
                };
            };
        };
    };
    get_execution_output_handler: {
        parameters: {
            query?: {
                
                line_offset?: number | null;
                
                line_limit?: number | null;
                
                byte_offset?: number | null;
                
                byte_limit?: number | null;
            };
            header?: never;
            path: {
                
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ExecutionOutput"];
                };
            };
            
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    version_handler: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
        };
    };
}
