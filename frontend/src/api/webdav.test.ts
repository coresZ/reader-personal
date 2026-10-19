import { describe, expect, it, vi } from 'vitest'
import {
  createWebdavFileBlob,
  decodeWebdavFileText,
  syncLegadoBookProgress,
  uploadFilesToWebdav,
  uploadLegadoBookProgress,
} from './webdav'
import { invokeEnvelope, invokeUpload } from './invoke'

vi.mock('./invoke', () => ({
  get: vi.fn(),
  post: vi.fn(),
  invokeEnvelope: vi.fn(),
  invokeRaw: vi.fn(),
  // 本文件的模块图当前不触达上传，但 mock 需与 invoke.ts 的导出保持一致，
  // 否则将来重构引入命名导入时会以 "invokeUpload is not a function" 报错。
  invokeUpload: vi.fn(),
  // webdav.ts 用它把 Rust 侧的字符串错误转成 Error，同样必须保留导出。
  toError: (value: unknown, fallback = '请求失败') =>
    value instanceof Error ? value : new Error(typeof value === 'string' && value.trim() ? value : fallback),
}))

describe('webdav binary file conversion', () => {
  it('decodes an old JSON backup with UTF-8 text intact', () => {
    const raw = '{"version":1,"bookName":"测试书目"}'
    const bytes = Array.from(new TextEncoder().encode(raw))

    expect(decodeWebdavFileText({ bytes })).toBe(raw)
  })

  it('creates a downloadable blob from the binary response', async () => {
    const raw = '备份内容'
    const bytes = new TextEncoder().encode(raw)
    const blob = createWebdavFileBlob({ bytes, content_type: 'application/json' })

    expect(blob.type).toBe('application/json')
    expect(await blob.text()).toBe(raw)
  })

  it('wraps Legado progress in the request object expected by Tauri', async () => {
    vi.mocked(invokeEnvelope).mockResolvedValue({ configured: true, uploaded: true })
    const config = {
      url: 'https://dav.example.test/',
      account: 'reader',
      password: 'secret',
      directory: 'legado',
    }
    const progress = {
      name: '测试书籍',
      author: '测试作者',
      durChapterIndex: 3,
      durChapterPos: 120,
      durChapterTime: 123456,
      durChapterTitle: '第四章',
    }

    await syncLegadoBookProgress(config, progress, true)

    expect(invokeEnvelope).toHaveBeenCalledWith('sync_legado_book_progress', {
      req: { config, progress, allowUpload: true },
    })
  })

  it('wraps direct Legado progress upload without comparison options', async () => {
    vi.mocked(invokeEnvelope).mockResolvedValue({ configured: true, uploaded: true })
    const config = {
      url: 'https://dav.example.test/',
      account: 'reader',
      password: 'secret',
      directory: 'legado',
    }
    const progress = {
      name: '测试书籍',
      author: '测试作者',
      durChapterIndex: 3,
      durChapterPos: 120,
      durChapterTime: 123456,
      durChapterTitle: '第四章',
    }

    await uploadLegadoBookProgress(config, progress)

    expect(invokeEnvelope).toHaveBeenCalledWith('upload_legado_book_progress', {
      req: { config, progress },
    })
  })
})

describe('uploadFilesToWebdav', () => {
  /** 多文件改为「逐文件调用」：每个文件一次 invokeUpload，字节走二进制通道，
   *  文件名与目标目录走 header（值经 encodeURIComponent 以承载中文）。 */
  it('uploads each file in one call with name and path headers', async () => {
    const calls: Array<{ command: string; headers: Record<string, string>; blob: Blob }> = []
    vi.mocked(invokeUpload).mockImplementation(async (command, headers, blob) => {
      calls.push({ command, headers, blob: blob as Blob })
      const name = decodeURIComponent(headers['x-reader-file-name'])
      return [{ name, size: 1, path: `/${name}`, lastModified: 0, isDirectory: false }]
    })

    const files = [
      { file: new Blob(['a']), name: '第一本.json' },
      { file: new Blob(['b']), name: 'book two.json' },
    ]
    const entries = await uploadFilesToWebdav(files, '/备份')

    expect(calls.map((c) => c.command)).toEqual([
      'upload_file_to_webdav',
      'upload_file_to_webdav',
    ])
    // 顺序与传入一致
    expect(calls.map((c) => c.headers['x-reader-file-name'])).toEqual([
      encodeURIComponent('第一本.json'),
      encodeURIComponent('book two.json'),
    ])
    // 目标目录同样走 header，中文目录可还原
    for (const call of calls) {
      expect(decodeURIComponent(call.headers['x-reader-webdav-path'])).toBe('/备份')
    }
    // 每个文件传的是自己的 Blob
    expect(calls.map((c) => c.blob)).toEqual(files.map((f) => f.file))
    // 返回值是各次结果的汇总
    expect(entries.map((e) => e.name)).toEqual(['第一本.json', 'book two.json'])
  })

  it('makes no call for an empty file list', async () => {
    vi.mocked(invokeUpload).mockClear()
    expect(await uploadFilesToWebdav([], '/')).toEqual([])
    expect(invokeUpload).not.toHaveBeenCalled()
  })
})
