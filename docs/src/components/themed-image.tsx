import Image from "next/image"

type ThemedImageProps = {
  lightSrc: string
  darkSrc: string
  alt: string
  width: number
  height: number
  caption?: string
}

export function ThemedImage({ lightSrc, darkSrc, alt, width, height, caption }: ThemedImageProps) {
  return (
    <figure className="my-6">
      <a
        href={lightSrc}
        target="_blank"
        rel="noopener noreferrer"
        className="block cursor-zoom-in dark:hidden"
      >
        <Image src={lightSrc} alt={alt} width={width} height={height} className="rounded-lg" />
        <span className="sr-only">Open full-size image in a new tab.</span>
      </a>
      <a
        href={darkSrc}
        target="_blank"
        rel="noopener noreferrer"
        className="hidden cursor-zoom-in dark:block"
      >
        <Image src={darkSrc} alt={alt} width={width} height={height} className="rounded-lg" />
        <span className="sr-only">Open full-size image in a new tab.</span>
      </a>
      {caption && (
        <figcaption className="mt-2 text-center text-sm text-gray-600 dark:text-gray-400">
          {caption}
        </figcaption>
      )}
    </figure>
  )
}
