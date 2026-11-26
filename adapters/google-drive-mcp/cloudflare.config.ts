export default {
  worker: {
    name: "renoa-google-drive-mcp",
    compatibilityDate: "2026-09-03",
    compatibilityFlags: ["nodejs_compat"],
    entrypoint: "src/index.ts",
    workersDev: false,
    observability: {
      enabled: true,
      headSamplingRate: 1,
      logs: {
        invocationLogs: true,
        headSamplingRate: 1,
      },
      traces: {
        enabled: true,
        headSamplingRate: 0.1,
      },
    },
    domains: ["drive.renoa.live"],
  },
};
