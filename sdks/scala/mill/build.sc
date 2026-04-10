import mill._
import mill.scalalib._

/**
 * Mill plugin build for zio-golem.
 *
 * This build is intentionally independent of the sbt build.
 *
 * Mill's own core libraries (scalalib, scalajslib) are available on the
 * classpath automatically when this build.sc is evaluated by Mill. The
 * GolemAutoRegister trait extends ScalaJSModule and references scalajslib API
 * types.
 */

object golemCodegen extends ScalaModule {
  def scalaVersion = "3.3.7"

  def sources = T.sources(
    millSourcePath / os.up / "codegen" / "src" / "main" / "scala"
  )

  def ivyDeps = Agg(
    ivy"org.scalameta::scalameta:4.14.7",
    ivy"com.lihaoyi::ujson:3.1.0"
  )
}

object zioGolemMill extends ScalaModule {
  def scalaVersion = "3.3.7"

  override def moduleDeps = Seq(golemCodegen)

  // Publishing coordinates (match the runtime modules)
  def artifactName = "zio-golem-mill"

  def sources = T.sources(millSourcePath / "src")

  def resources = T.sources(millSourcePath / "resources")
}
