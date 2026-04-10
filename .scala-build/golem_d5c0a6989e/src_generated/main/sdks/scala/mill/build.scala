package sdks.scala.mill


final class build$_ {
def args = build_sc.args$
def scriptPath = """sdks/scala/mill/build.sc"""
/*<script>*/
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

/*</script>*/ /*<generated>*//*</generated>*/
}

object build_sc {
  private var args$opt0 = Option.empty[Array[String]]
  def args$set(args: Array[String]): Unit = {
    args$opt0 = Some(args)
  }
  def args$opt: Option[Array[String]] = args$opt0
  def args$: Array[String] = args$opt.getOrElse {
    sys.error("No arguments passed to this script")
  }

  lazy val script = new build$_

  def main(args: Array[String]): Unit = {
    args$set(args)
    val _ = script.hashCode() // hashCode to clear scalac warning about pure expression in statement position
  }
}

export build_sc.script as `build`

