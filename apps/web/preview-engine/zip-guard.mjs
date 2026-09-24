import JSZip from 'jszip';
export async function validateOfficeZip(buffer) {
  if (!(buffer instanceof ArrayBuffer) || buffer.byteLength > 8 * 1024 * 1024) throw new Error('Office 文件超过 8 MiB 预览限制。');
  const zip = await JSZip.loadAsync(buffer); const files = Object.values(zip.files);
  if (files.length > 4096) throw new Error('Office 包内文件数量超限。');
  let total = 0;
  for (const file of files) {
    if (file.dir) continue;
    const size = file._data?.uncompressedSize;
    if (!Number.isSafeInteger(size) || size < 0 || size > 16 * 1024 * 1024 || /vbaProject\.bin$/i.test(file.name)) throw new Error('文件包含不支持或超限的内容。');
    total += size; if (total > 48 * 1024 * 1024) throw new Error('Office 解压内容超过 48 MiB 限制。');
  }
}
